use crate::module::collect_ident_uses;
use crate::scope::{CompiledUnit, LoopScope, ParentScopeInfo};
use aluka_bytecode::{Constant, Instr, Op, TryEntry};
use aluka_parser::ast::{
    Expr, Program, PropKey, PropValue, SpannedStmt, Stmt, VarKind, VarPattern,
};

/// `PushInt` 立即值能表示的上界（24 位操作数）。超过它的数值走常量池。
const MAX_IMMEDIATE: f64 = ((1u32 << 24) - 1) as f64;

/// 把语法树编译成字节码产物单元。
#[must_use]
pub fn compile(program: &Program) -> CompiledUnit {
    let mut unit = CompiledUnit::default();
    let num_stmts = program.body.len();
    for (i, stmt) in program.body.iter().enumerate() {
        let is_last = i == num_stmts - 1;
        compile_stmt(stmt, &mut unit, is_last);
    }
    // 若最后一条语句没有留下返回值，补充 ReturnUndef
    if unit.code.is_empty()
        || !matches!(
            unit.code.last().map(|i| i.op),
            Some(Op::Return | Op::ReturnUndef)
        )
    {
        unit.code.push(Instr::new(Op::Return, 0));
    }
    unit
}

fn compile_bind_pattern(pattern: &VarPattern, src_slot: usize, unit: &mut CompiledUnit) {
    match pattern {
        VarPattern::Ident(name) => {
            if !name.is_empty() {
                let slot = if let Some(&s) = unit.symbol_map.get(name) {
                    s
                } else {
                    let s = unit.locals;
                    unit.locals += 1;
                    unit.symbol_map.insert(name.clone(), s);
                    s
                };
                unit.code.push(Instr::new(Op::LoadLocal, src_slot as u32));
                unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
            }
        }
        VarPattern::Array(elements) => {
            for (i, elem) in elements.iter().enumerate() {
                if elem.name.is_empty() {
                    continue;
                }
                let slot = if let Some(&s) = unit.symbol_map.get(&elem.name) {
                    s
                } else {
                    let s = unit.locals;
                    unit.locals += 1;
                    unit.symbol_map.insert(elem.name.clone(), s);
                    s
                };

                if elem.is_rest {
                    unit.code.push(Instr::new(Op::LoadLocal, src_slot as u32));
                    unit.code.push(Instr::new(Op::PushInt, i as u32));
                    let slice_idx = add_constant(unit, Constant::String("slice".to_owned()));
                    let operand = (1u32 << 16) | (slice_idx & 0xFFFF);
                    unit.code.push(Instr::new(Op::CallMethod, operand));
                    unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                } else {
                    unit.code.push(Instr::new(Op::LoadLocal, src_slot as u32));
                    unit.code.push(Instr::new(Op::PushInt, i as u32));
                    unit.code.push(Instr::new(Op::GetElem, 0));
                    if let Some(ref def_expr) = elem.default_value {
                        let jmp_idx = emit_jump(unit, Op::JmpNullishKeep);
                        compile_expr(def_expr, unit);
                        let end_idx = unit.code.len();
                        backpatch_jump(unit, jmp_idx, end_idx);
                    }
                    unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                }
            }
        }
        VarPattern::Object(props) => {
            for prop in props {
                let prop_tmp = unit.locals;
                unit.locals += 1;
                unit.code.push(Instr::new(Op::LoadLocal, src_slot as u32));
                let name_idx = add_constant(unit, Constant::String(prop.key.clone()));
                unit.code.push(Instr::new(Op::GetProp, name_idx));
                if let Some(ref def_expr) = prop.default_value {
                    let jmp_idx = emit_jump(unit, Op::JmpNullishKeep);
                    compile_expr(def_expr, unit);
                    let end_idx = unit.code.len();
                    backpatch_jump(unit, jmp_idx, end_idx);
                }
                unit.code.push(Instr::new(Op::StoreLocal, prop_tmp as u32));
                compile_bind_pattern(&prop.value, prop_tmp, unit);
            }
        }
    }
}

/// 循环回边行表补登记：cond/update 指令物理位于 body 之后，
/// 不补记会把回边执行错误归属到 body 内的最后一条语句（实测
/// else 体被多计一次）。
fn record_loop_line(unit: &mut CompiledUnit, line: u32) {
    if unit.line_coverage {
        let pc = unit.code.len() as u32;
        if !matches!(unit.line_table.last(), Some((_, l)) if *l == line) {
            unit.line_table.push((pc, line));
        }
    }
}

pub(crate) fn compile_stmt(s: &SpannedStmt, unit: &mut CompiledUnit, is_last: bool) {
    // LCOV 行覆盖：语句起始（pc, line）登记进当前函数行表（覆盖模式才记录，
    // 保证默认编译产物与关闭态逐字节一致）。Block 包装语句不登记——它自身
    // 零宽（无指令），内部语句自会登记；否则零宽项会吞掉迁移计数并产生
    // 指向 `}`/`else` 行的假 DA 条目。
    if unit.line_coverage && !matches!(s.stmt, Stmt::Block(_)) {
        // 连续同行条目合并（for-init/for 头、else 包装等零宽重复）——
        // 重复条目会让迁移计数把同一语句执行计成多次
        let dup_last = matches!(unit.line_table.last(), Some((_, l)) if *l == s.line);
        if !dup_last {
            unit.line_table.push((unit.code.len() as u32, s.line));
        }
    }
    let stmt = &s.stmt;
    match stmt {
        Stmt::Expr(expr) => {
            compile_expr(expr, unit);
            if !is_last {
                // 非末尾纯表达式语句，求值后弹栈保持栈平衡
                unit.code.push(Instr::new(Op::Pop, 0));
            }
        }
        Stmt::VarDecl {
            name,
            init,
            kind: var_kind,
        } => {
            // 隐式全局模式（eval 全局作用域求值）：var 声明直接落全局表，
            // 不注册局部符号——后续标识符引用经 LoadGlobal 命中同一绑定
            // （let/const 不走此路径：eval 内块级声明保持求值域局部）
            if unit.implicit_globals && *var_kind == VarKind::Var {
                if let Some(init_expr) = init {
                    compile_expr(init_expr, unit);
                } else {
                    unit.code.push(Instr::new(Op::PushUndefined, 0));
                }
                let name_idx = add_constant(unit, Constant::String(name.clone()));
                unit.code.push(Instr::new(Op::StoreGlobal, name_idx));
                if is_last {
                    unit.code.push(Instr::new(Op::PushUndefined, 0));
                }
                return;
            }
            let slot = if *var_kind != VarKind::Var && unit.block_depth > 0 {
                // let/const 在嵌套块级作用域中：分配新槽并记录遮蔽
                let s = unit.locals;
                unit.locals += 1;
                let prev = unit.symbol_map.insert(name.clone(), s);
                unit.scope_shadow_log.push((name.clone(), prev));
                s
            } else if let Some(&s) = unit.symbol_map.get(name) {
                // 顶层（或 var）且已预分配槽位：直接复用
                s
            } else {
                // 顶层未预分配：分配新槽
                let s = unit.locals;
                unit.locals += 1;
                unit.symbol_map.insert(name.clone(), s);
                s
            };

            if let Some(init_expr) = init {
                compile_expr(init_expr, unit);
            } else {
                unit.code.push(Instr::new(Op::PushUndefined, 0));
            }

            unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
            if is_last {
                unit.code.push(Instr::new(Op::PushUndefined, 0));
            }
        }
        Stmt::MultiVarDecl {
            kind: var_kind,
            decls,
        } => {
            for (name, init) in decls {
                let slot = if *var_kind != VarKind::Var && unit.block_depth > 0 {
                    let s = unit.locals;
                    unit.locals += 1;
                    let prev = unit.symbol_map.insert(name.clone(), s);
                    unit.scope_shadow_log.push((name.clone(), prev));
                    s
                } else if let Some(&s) = unit.symbol_map.get(name) {
                    s
                } else {
                    let s = unit.locals;
                    unit.locals += 1;
                    unit.symbol_map.insert(name.clone(), s);
                    s
                };
                if let Some(init_expr) = init {
                    compile_expr(init_expr, unit);
                } else {
                    unit.code.push(Instr::new(Op::PushUndefined, 0));
                }
                unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
            }
            if is_last {
                unit.code.push(Instr::new(Op::PushUndefined, 0));
            }
        }
        Stmt::DestructureDecl { pattern, init } => {
            compile_expr(init, unit);
            let tmp_slot = unit.locals;
            unit.locals += 1;
            unit.code.push(Instr::new(Op::StoreLocal, tmp_slot as u32));

            compile_bind_pattern(pattern, tmp_slot, unit);

            if is_last {
                unit.code.push(Instr::new(Op::PushUndefined, 0));
            }
        }
        Stmt::Block(stmts) => {
            unit.block_depth += 1;
            let scope_mark = unit.scope_shadow_log.len();
            let n = stmts.len();
            for (j, s) in stmts.iter().enumerate() {
                compile_stmt(s, unit, is_last && j == n - 1);
            }
            // 块结束：恢复被 let/const 遮蔽的外层绑定（后进先出）
            while unit.scope_shadow_log.len() > scope_mark {
                if let Some((name, prev)) = unit.scope_shadow_log.pop() {
                    match prev {
                        Some(slot) => {
                            unit.symbol_map.insert(name, slot);
                        }
                        None => {
                            unit.symbol_map.remove(&name);
                        }
                    }
                }
            }
            unit.block_depth -= 1;
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
        } => {
            compile_expr(cond, unit);
            let jmp_false_idx = emit_jump(unit, Op::JmpFalsePop);
            compile_stmt(then_branch, unit, false);
            if let Some(else_stmt) = else_branch {
                let jmp_end_idx = emit_jump(unit, Op::Jmp);
                let else_start = unit.code.len();
                backpatch_jump(unit, jmp_false_idx, else_start);
                compile_stmt(else_stmt, unit, false);
                let end_idx = unit.code.len();
                backpatch_jump(unit, jmp_end_idx, end_idx);
            } else {
                let end_idx = unit.code.len();
                backpatch_jump(unit, jmp_false_idx, end_idx);
            }
            if is_last {
                unit.code.push(Instr::new(Op::PushUndefined, 0));
            }
        }
        Stmt::While { cond, body } => {
            let loop_start = unit.code.len();
            compile_expr(cond, unit);
            let exit_jmp_idx = emit_jump(unit, Op::JmpFalsePop);

            unit.loop_stack.push(LoopScope::default());
            compile_stmt(body, unit, false);
            let scope = unit.loop_stack.pop().unwrap_or_default();

            for c_jmp in scope.continue_jumps {
                backpatch_jump(unit, c_jmp, loop_start);
            }

            let loop_jmp_idx = emit_jump(unit, Op::Jmp);
            backpatch_jump(unit, loop_jmp_idx, loop_start);
            let loop_end = unit.code.len();
            backpatch_jump(unit, exit_jmp_idx, loop_end);

            for b_jmp in scope.break_jumps {
                backpatch_jump(unit, b_jmp, loop_end);
            }

            if is_last {
                unit.code.push(Instr::new(Op::PushUndefined, 0));
            }
        }
        Stmt::DoWhile { body, cond } => {
            let loop_start = unit.code.len();

            unit.loop_stack.push(LoopScope::default());
            compile_stmt(body, unit, false);
            let scope = unit.loop_stack.pop().unwrap_or_default();

            let continue_target = unit.code.len();
            for c_jmp in scope.continue_jumps {
                backpatch_jump(unit, c_jmp, continue_target);
            }

            compile_expr(cond, unit);
            let back_jmp = emit_jump(unit, Op::JmpTruePop);
            backpatch_jump(unit, back_jmp, loop_start);

            let loop_end = unit.code.len();
            for b_jmp in scope.break_jumps {
                backpatch_jump(unit, b_jmp, loop_end);
            }

            if is_last {
                unit.code.push(Instr::new(Op::PushUndefined, 0));
            }
        }
        Stmt::For {
            init,
            cond,
            update,
            body,
        } => {
            let let_var = match init {
                Some(b) => match &b.stmt {
                    Stmt::VarDecl { name, .. } => Some(name.clone()),
                    _ => None,
                },
                None => None,
            };

            let captured = if let Some(ref name) = let_var {
                stmt_has_closure_capturing(body, name)
            } else {
                false
            };

            if captured {
                let name = let_var.unwrap();
                if let Some(init_stmt) = init {
                    compile_stmt(init_stmt, unit, false);
                }
                let head_slot = *unit.symbol_map.get(&name).unwrap();
                let iter_slot = unit.locals;
                unit.locals += 1;

                let loop_start = unit.code.len();
                let exit_jmp = if let Some(cond_expr) = cond {
                    compile_expr(cond_expr, unit);
                    Some(emit_jump(unit, Op::JmpFalsePop))
                } else {
                    None
                };

                // 进入当前迭代：遮蔽为 iter_slot，并同步 head_slot -> iter_slot
                unit.symbol_map.insert(name.clone(), iter_slot);
                unit.code.push(Instr::new(Op::LoadLocal, head_slot as u32));
                unit.code.push(Instr::new(Op::StoreLocal, iter_slot as u32));

                unit.loop_stack.push(LoopScope::default());
                compile_stmt(body, unit, false);
                let scope = unit.loop_stack.pop().unwrap_or_default();

                let continue_target = unit.code.len();
                for c_jmp in scope.continue_jumps {
                    backpatch_jump(unit, c_jmp, continue_target);
                }

                // 迭代结束：关闭捕获的 Upvalue，产生独立副本
                unit.code
                    .push(Instr::new(Op::CloseUpvalues, iter_slot as u32));

                if let Some(update_expr) = update {
                    compile_expr(update_expr, unit);
                    unit.code.push(Instr::new(Op::Pop, 0));
                }

                // 更新完成后同步回 head_slot
                unit.code.push(Instr::new(Op::LoadLocal, iter_slot as u32));
                unit.code.push(Instr::new(Op::StoreLocal, head_slot as u32));

                let loop_back = emit_jump(unit, Op::Jmp);
                backpatch_jump(unit, loop_back, loop_start);

                let break_cleanup_target = unit.code.len();
                unit.code
                    .push(Instr::new(Op::CloseUpvalues, iter_slot as u32));

                let loop_end = unit.code.len();
                if let Some(exit_jmp_idx) = exit_jmp {
                    backpatch_jump(unit, exit_jmp_idx, loop_end);
                }
                for b_jmp in scope.break_jumps {
                    backpatch_jump(unit, b_jmp, break_cleanup_target);
                }

                // 退出循环后恢复外层符号映射
                unit.symbol_map.insert(name, head_slot);

                if is_last {
                    unit.code.push(Instr::new(Op::PushUndefined, 0));
                }
            } else {
                if let Some(init_stmt) = init {
                    compile_stmt(init_stmt, unit, false);
                }
                let cond_start = unit.code.len();
                let exit_jmp = if let Some(cond_expr) = cond {
                    compile_expr(cond_expr, unit);
                    Some(emit_jump(unit, Op::JmpFalsePop))
                } else {
                    None
                };

                unit.loop_stack.push(LoopScope::default());
                compile_stmt(body, unit, false);
                let scope = unit.loop_stack.pop().unwrap_or_default();

                let update_start = unit.code.len();
                for c_jmp in scope.continue_jumps {
                    backpatch_jump(unit, c_jmp, update_start);
                }

                record_loop_line(unit, s.line);
                if let Some(update_expr) = update {
                    compile_expr(update_expr, unit);
                    unit.code.push(Instr::new(Op::Pop, 0));
                }

                let loop_back = emit_jump(unit, Op::Jmp);
                backpatch_jump(unit, loop_back, cond_start);

                let loop_end = unit.code.len();
                if let Some(exit_jmp_idx) = exit_jmp {
                    backpatch_jump(unit, exit_jmp_idx, loop_end);
                }
                for b_jmp in scope.break_jumps {
                    backpatch_jump(unit, b_jmp, loop_end);
                }

                if is_last {
                    unit.code.push(Instr::new(Op::PushUndefined, 0));
                }
            }
        }
        Stmt::ForIn {
            pattern,
            right,
            body,
        } => {
            compile_expr(right, unit);
            let tmp_src = unit.locals;
            unit.locals += 1;
            unit.code.push(Instr::new(Op::StoreLocal, tmp_src as u32));

            let tmp_keys = unit.locals;
            unit.locals += 1;
            unit.code.push(Instr::new(Op::LoadLocal, tmp_src as u32));
            unit.code.push(Instr::new(Op::EnumKeys, 0));
            unit.code.push(Instr::new(Op::StoreLocal, tmp_keys as u32));

            let len_const = add_constant(unit, Constant::String("length".to_owned()));
            let tmp_len = unit.locals;
            unit.locals += 1;
            unit.code.push(Instr::new(Op::LoadLocal, tmp_keys as u32));
            unit.code.push(Instr::new(Op::GetProp, len_const));
            unit.code.push(Instr::new(Op::StoreLocal, tmp_len as u32));

            let tmp_idx = unit.locals;
            unit.locals += 1;
            unit.code.push(Instr::new(Op::PushInt, 0));
            unit.code.push(Instr::new(Op::StoreLocal, tmp_idx as u32));

            let loop_start = unit.code.len();
            unit.code.push(Instr::new(Op::LoadLocal, tmp_idx as u32));
            unit.code.push(Instr::new(Op::LoadLocal, tmp_len as u32));
            unit.code.push(Instr::new(Op::Lt, 0));
            let exit_jmp = emit_jump(unit, Op::JmpFalsePop);

            unit.loop_stack.push(LoopScope::default());

            unit.code.push(Instr::new(Op::LoadLocal, tmp_keys as u32));
            unit.code.push(Instr::new(Op::LoadLocal, tmp_idx as u32));
            unit.code.push(Instr::new(Op::GetElem, 0));

            match pattern {
                VarPattern::Ident(name) => {
                    let slot = if let Some(s) = unit.symbol_map.get(name) {
                        *s
                    } else {
                        let s = unit.locals;
                        unit.locals += 1;
                        unit.symbol_map.insert(name.clone(), s);
                        s
                    };
                    unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                }
                VarPattern::Array(_) | VarPattern::Object(_) => {
                    let tmp_slot = unit.locals;
                    unit.locals += 1;
                    unit.code.push(Instr::new(Op::StoreLocal, tmp_slot as u32));
                    compile_bind_pattern(pattern, tmp_slot, unit);
                }
            }

            compile_stmt(body, unit, false);

            let scope = unit.loop_stack.pop().unwrap_or_default();
            let continue_target = unit.code.len();
            for c_jmp in scope.continue_jumps {
                backpatch_jump(unit, c_jmp, continue_target);
            }

            unit.code.push(Instr::new(Op::LoadLocal, tmp_idx as u32));
            unit.code.push(Instr::new(Op::PushInt, 1));
            unit.code.push(Instr::new(Op::Add, 0));
            unit.code.push(Instr::new(Op::StoreLocal, tmp_idx as u32));

            let loop_back = emit_jump(unit, Op::Jmp);
            backpatch_jump(unit, loop_back, loop_start);

            let loop_end = unit.code.len();
            backpatch_jump(unit, exit_jmp, loop_end);
            for b_jmp in scope.break_jumps {
                backpatch_jump(unit, b_jmp, loop_end);
            }

            if is_last {
                unit.code.push(Instr::new(Op::PushUndefined, 0));
            }
        }
        Stmt::ForOf {
            is_await,
            pattern,
            right,
            body,
        } => {
            compile_expr(right, unit);
            if *is_await {
                unit.code.push(Instr::new(Op::GetAsyncIterator, 0));
            } else {
                unit.code.push(Instr::new(Op::GetIterator, 0));
            }
            let tmp_iter = unit.locals;
            unit.locals += 1;
            unit.code.push(Instr::new(Op::StoreLocal, tmp_iter as u32));

            let tmp_result = unit.locals;
            unit.locals += 1;

            let name_next = add_constant(unit, Constant::String("next".to_owned()));
            let name_done = add_constant(unit, Constant::String("done".to_owned()));
            let name_value = add_constant(unit, Constant::String("value".to_owned()));

            let loop_start = unit.code.len();

            unit.code.push(Instr::new(Op::LoadLocal, tmp_iter as u32));
            unit.code.push(Instr::new(Op::CallMethod, name_next));
            if *is_await {
                unit.code.push(Instr::new(Op::Await, 0));
            }
            unit.code
                .push(Instr::new(Op::StoreLocal, tmp_result as u32));

            unit.code.push(Instr::new(Op::LoadLocal, tmp_result as u32));
            unit.code.push(Instr::new(Op::GetProp, name_done));
            let exit_jmp = emit_jump(unit, Op::JmpTruePop);

            unit.loop_stack.push(LoopScope::default());

            unit.code.push(Instr::new(Op::LoadLocal, tmp_result as u32));
            unit.code.push(Instr::new(Op::GetProp, name_value));

            match pattern {
                VarPattern::Ident(name) => {
                    let slot = if let Some(s) = unit.symbol_map.get(name) {
                        *s
                    } else {
                        let s = unit.locals;
                        unit.locals += 1;
                        unit.symbol_map.insert(name.clone(), s);
                        s
                    };
                    unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                }
                VarPattern::Array(_) | VarPattern::Object(_) => {
                    let tmp_slot = unit.locals;
                    unit.locals += 1;
                    unit.code.push(Instr::new(Op::StoreLocal, tmp_slot as u32));
                    compile_bind_pattern(pattern, tmp_slot, unit);
                }
            }

            compile_stmt(body, unit, false);

            let scope = unit.loop_stack.pop().unwrap_or_default();
            let continue_target = unit.code.len();
            for c_jmp in scope.continue_jumps {
                backpatch_jump(unit, c_jmp, continue_target);
            }

            let loop_back = emit_jump(unit, Op::Jmp);
            backpatch_jump(unit, loop_back, loop_start);

            let loop_end = unit.code.len();
            backpatch_jump(unit, exit_jmp, loop_end);
            for b_jmp in scope.break_jumps {
                backpatch_jump(unit, b_jmp, loop_end);
            }

            if is_last {
                unit.code.push(Instr::new(Op::PushUndefined, 0));
            }
        }
        Stmt::Break => {
            let jmp = emit_jump(unit, Op::Jmp);
            if let Some(scope) = unit.loop_stack.last_mut() {
                scope.break_jumps.push(jmp);
            }
        }
        Stmt::Continue => {
            let jmp = emit_jump(unit, Op::Jmp);
            if let Some(scope) = unit.loop_stack.last_mut() {
                scope.continue_jumps.push(jmp);
            }
        }
        Stmt::Return(maybe_expr) => {
            if let Some(expr) = maybe_expr {
                compile_expr(expr, unit);
                unit.code.push(Instr::new(Op::Return, 0));
            } else {
                unit.code.push(Instr::new(Op::ReturnUndef, 0));
            }
        }
        Stmt::Throw(expr) => {
            compile_expr(expr, unit);
            unit.code.push(Instr::new(Op::Throw, 0));
        }
        Stmt::Try {
            body,
            catch_param,
            catch_body,
            finally_body,
        } => {
            let try_idx = unit.try_table.len();
            let start_pc = (unit.code.len() * 4) as u32;
            unit.try_table.push(TryEntry {
                start_pc,
                end_pc: 0,
                catch_pc: 0,
                catch_end_pc: 0,
                finally_pc: 0,
                finally_end_pc: 0,
                has_catch: catch_body.is_some(),
                has_finally: finally_body.is_some(),
            });

            unit.code.push(Instr::new(Op::TryEnter, try_idx as u32));
            compile_stmt(body, unit, false);
            let end_pc = (unit.code.len() * 4) as u32;
            unit.try_table[try_idx].end_pc = end_pc;
            unit.code.push(Instr::new(Op::TryExit, try_idx as u32));

            let jmp_over_catch = emit_jump(unit, Op::Jmp);

            if let Some(cb) = catch_body {
                let catch_pc = (unit.code.len() * 4) as u32;
                unit.try_table[try_idx].catch_pc = catch_pc;
                if let Some(param_name) = catch_param {
                    let slot = if let Some(&s) = unit.symbol_map.get(param_name) {
                        s
                    } else {
                        let s = unit.locals;
                        unit.locals += 1;
                        unit.symbol_map.insert(param_name.clone(), s);
                        s
                    };
                    unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                } else {
                    unit.code.push(Instr::new(Op::Pop, 0));
                }
                compile_stmt(cb, unit, false);
                let catch_end_pc = (unit.code.len() * 4) as u32;
                unit.try_table[try_idx].catch_end_pc = catch_end_pc;
                unit.code.push(Instr::new(Op::TryExit, try_idx as u32));
            }

            let after_catch = unit.code.len();
            backpatch_jump(unit, jmp_over_catch, after_catch);

            if let Some(fb) = finally_body {
                let finally_pc = (unit.code.len() * 4) as u32;
                unit.try_table[try_idx].finally_pc = finally_pc;
                compile_stmt(fb, unit, false);
                let finally_end_pc = (unit.code.len() * 4) as u32;
                unit.try_table[try_idx].finally_end_pc = finally_end_pc;
                unit.code
                    .push(Instr::new(Op::TryExitFinally, try_idx as u32));
            }

            if is_last {
                unit.code.push(Instr::new(Op::PushUndefined, 0));
            }
        }
        Stmt::Switch {
            discriminant,
            cases,
        } => {
            compile_expr(discriminant, unit);
            let disc_slot = unit.locals;
            unit.locals += 1;
            unit.code.push(Instr::new(Op::StoreLocal, disc_slot as u32));

            unit.loop_stack.push(LoopScope::default());

            let mut case_jumps = Vec::new();
            let mut default_jump: Option<usize> = None;

            for case in cases {
                if let Some(ref test) = case.test {
                    unit.code.push(Instr::new(Op::LoadLocal, disc_slot as u32));
                    compile_expr(test, unit);
                    unit.code.push(Instr::new(Op::StrictEq, 0));
                    let jmp = emit_jump(unit, Op::JmpTruePop);
                    case_jumps.push(jmp);
                } else {
                    let jmp = emit_jump(unit, Op::Jmp);
                    default_jump = Some(jmp);
                }
            }

            let fall_jmp = emit_jump(unit, Op::Jmp);

            let mut body_pcs = Vec::with_capacity(cases.len());
            for case in cases {
                body_pcs.push(unit.code.len());
                for stmt in &case.consequent {
                    compile_stmt(stmt, unit, false);
                }
            }

            let mut idx = 0;
            for (i, case) in cases.iter().enumerate() {
                if case.test.is_none() {
                    if let Some(def_jmp) = default_jump {
                        backpatch_jump(unit, def_jmp, body_pcs[i]);
                    }
                } else {
                    let pc = case_jumps[idx];
                    backpatch_jump(unit, pc, body_pcs[i]);
                    idx += 1;
                }
            }

            let end_pc = unit.code.len();
            backpatch_jump(unit, fall_jmp, end_pc);

            let scope = unit.loop_stack.pop().unwrap_or_default();
            for b_jmp in scope.break_jumps {
                backpatch_jump(unit, b_jmp, end_pc);
            }

            if is_last {
                unit.code.push(Instr::new(Op::PushUndefined, 0));
            }
        }
        Stmt::Function(_) | Stmt::Class { .. } => {
            // 类与顶级函数声明在 compile_module 中提取并装配为独立的 FuncTemplate / ClassTemplate
            if is_last {
                unit.code.push(Instr::new(Op::PushUndefined, 0));
            }
        }
        Stmt::Import(_) => {
            if is_last {
                unit.code.push(Instr::new(Op::PushUndefined, 0));
            }
        }
        Stmt::Export(export_decl) => match export_decl {
            aluka_parser::ast::ExportDecl::Named {
                decl: Some(inner), ..
            } => {
                compile_stmt(inner, unit, is_last);
            }
            aluka_parser::ast::ExportDecl::Default(expr) => {
                compile_expr(expr, unit);
                if !is_last {
                    unit.code.push(Instr::new(Op::Pop, 0));
                }
            }
            _ => {
                if is_last {
                    unit.code.push(Instr::new(Op::PushUndefined, 0));
                }
            }
        },
    }
}

/// 发射占位跳转指令，返回该跳转指令在代码流中的索引。
pub fn emit_jump(unit: &mut CompiledUnit, op: Op) -> usize {
    let idx = unit.code.len();
    unit.code.push(Instr::new(op, 0));
    idx
}

/// 回填跳转指令的有符号相对字节偏移。
pub fn backpatch_jump(unit: &mut CompiledUnit, jump_idx: usize, target_idx: usize) {
    let offset_bytes = (target_idx as i32 - (jump_idx as i32 + 1)) * 4;
    let operand = (offset_bytes as u32) & 0x00FF_FFFF;
    unit.code[jump_idx].operand = operand;
}

fn compile_args_array(args: &[Expr], unit: &mut CompiledUnit) {
    unit.code.push(Instr::new(Op::BuildArray, 0));
    for a in args {
        if let Expr::Spread(inner) = a {
            compile_expr(inner, unit);
            unit.code.push(Instr::new(Op::ArraySpread, 0));
        } else {
            compile_expr(a, unit);
            unit.code.push(Instr::new(Op::ArrayPush, 0));
        }
    }
}

fn compile_template_literal(quasis: &[String], exprs: &[Expr], unit: &mut CompiledUnit) {
    if exprs.is_empty() {
        let text = quasis.first().cloned().unwrap_or_default();
        let idx = add_constant(unit, Constant::String(text));
        unit.code.push(Instr::new(Op::PushConst, idx));
        return;
    }

    let first_text = quasis.first().cloned().unwrap_or_default();
    let idx = add_constant(unit, Constant::String(first_text));
    unit.code.push(Instr::new(Op::PushConst, idx));

    for (i, expr) in exprs.iter().enumerate() {
        compile_expr(expr, unit);
        unit.code.push(Instr::new(Op::Add, 0));

        let quasi_text = quasis.get(i + 1).cloned().unwrap_or_default();
        if !quasi_text.is_empty() {
            let q_idx = add_constant(unit, Constant::String(quasi_text));
            unit.code.push(Instr::new(Op::PushConst, q_idx));
            unit.code.push(Instr::new(Op::Add, 0));
        }
    }
}

/// 标记模板的实参序列：strings 数组（dup 设 `raw`）+ 插值表达式。
fn emit_tagged_args(quasis: &[String], raws: &[String], exprs: &[Expr], unit: &mut CompiledUnit) {
    let cooked: Vec<Expr> = quasis.iter().map(|q| Expr::String(q.clone())).collect();
    compile_expr(&Expr::Array(cooked), unit);
    unit.code.push(Instr::new(Op::Dup, 0));
    let key_idx = add_constant(unit, Constant::String("raw".to_owned()));
    unit.code.push(Instr::new(Op::PushConst, key_idx));
    let raw_exprs: Vec<Expr> = raws.iter().map(|r| Expr::String(r.clone())).collect();
    compile_expr(&Expr::Array(raw_exprs), unit);
    // 弹 val(raws)+key("raw")，peek 设定 dup.raw，dup 留栈
    unit.code.push(Instr::new(Op::SetPropComputedObj, 0));
    unit.code.push(Instr::new(Op::Pop, 0));
    for e in exprs {
        compile_expr(e, unit);
    }
}

pub(crate) fn add_constant(unit: &mut CompiledUnit, c: Constant) -> u32 {
    if let Some(pos) = unit.constants.iter().position(|x| *x == c) {
        pos as u32
    } else {
        let idx = unit.constants.len() as u32;
        unit.constants.push(c);
        idx
    }
}

pub(crate) fn compile_expr(expr: &Expr, unit: &mut CompiledUnit) {
    match expr {
        Expr::Number(n) => {
            // `-0.0 >= 0.0` 为真，但 `PushInt` 会把负零的符号丢掉（`1 / -0` 应为
            // `-Infinity`），故负零必须走常量池以保留 f64 位型。
            if *n >= 0.0
                && !(n.is_sign_negative() && *n == 0.0)
                && n.fract() == 0.0
                && *n <= MAX_IMMEDIATE
            {
                unit.code.push(Instr::new(Op::PushInt, *n as u32));
            } else if *n < 0.0 && n.fract() == 0.0 && -*n <= MAX_IMMEDIATE {
                unit.code.push(Instr::new(Op::PushNegInt, (-*n) as u32));
            } else {
                let idx = add_constant(unit, Constant::Number(*n));
                unit.code.push(Instr::new(Op::PushConst, idx));
            }
        }
        Expr::BigInt(b) => {
            let idx = add_constant(unit, Constant::BigInt(b.clone()));
            unit.code.push(Instr::new(Op::PushConst, idx));
        }
        Expr::Boolean(true) => {
            unit.code.push(Instr::new(Op::PushTrue, 0));
        }
        Expr::Boolean(false) => {
            unit.code.push(Instr::new(Op::PushFalse, 0));
        }
        Expr::Null => {
            unit.code.push(Instr::new(Op::PushNull, 0));
        }
        Expr::Undefined => {
            unit.code.push(Instr::new(Op::PushUndefined, 0));
        }
        Expr::This => {
            unit.code.push(Instr::new(Op::LoadLocal, 0));
        }
        Expr::String(s) => {
            let idx = add_constant(unit, Constant::String(s.clone()));
            unit.code.push(Instr::new(Op::PushConst, idx));
        }
        Expr::Ident(name) => {
            if let Some(&slot) = unit.symbol_map.get(name) {
                unit.code.push(Instr::new(Op::LoadLocal, slot as u32));
            } else if let Some(&uv_idx) = unit.upvalue_map.get(name) {
                unit.code.push(Instr::new(Op::LoadUpvalue, uv_idx as u32));
            } else {
                let name_idx = add_constant(unit, Constant::String(name.clone()));
                unit.code.push(Instr::new(Op::LoadGlobal, name_idx));
            }
        }
        Expr::Assign { name, value } => {
            compile_expr(value, unit);
            if let Some(&slot) = unit.symbol_map.get(name) {
                unit.code.push(Instr::new(Op::Dup, 0));
                unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
            } else if let Some(&uv_idx) = unit.upvalue_map.get(name) {
                unit.code.push(Instr::new(Op::Dup, 0));
                unit.code.push(Instr::new(Op::StoreUpvalue, uv_idx as u32));
            } else {
                // 未声明名赋值落全局表（与 Ident 读取的 LoadGlobal 对称）。
                // 禁止「写自动建局部槽、读走全局」的不对称：CJS 注入名
                // （exports/module/require 等）经此写入全局后，模块内嵌套
                // 函数的 LoadGlobal 才能读到新值（debug.js `exports =
                // module.exports = createDebug` 后子函数引用 exports.colors
                // 即依赖此对称性）；sloppy 脚本的隐式全局赋值同理。
                let name_idx = add_constant(unit, Constant::String(name.clone()));
                unit.code.push(Instr::new(Op::Dup, 0));
                unit.code.push(Instr::new(Op::StoreGlobal, name_idx));
            }
        }
        Expr::Unary { op, expr } => {
            if op == "delete" {
                match &**expr {
                    Expr::Member { obj, prop } => {
                        compile_expr(obj, unit);
                        let name_idx = add_constant(unit, Constant::String(prop.clone()));
                        unit.code.push(Instr::new(Op::DelProp, name_idx));
                    }
                    Expr::Index { obj, index } => {
                        compile_expr(obj, unit);
                        compile_expr(index, unit);
                        unit.code.push(Instr::new(Op::DelElem, 0));
                    }
                    other => {
                        compile_expr(other, unit);
                        unit.code.push(Instr::new(Op::Pop, 0));
                        unit.code.push(Instr::new(Op::PushTrue, 0));
                    }
                }
            } else if op == "void" {
                compile_expr(expr, unit);
                unit.code.push(Instr::new(Op::Pop, 0));
                unit.code.push(Instr::new(Op::PushUndefined, 0));
            } else if op == "typeof" {
                compile_expr(expr, unit);
                unit.code.push(Instr::new(Op::Typeof, 0));
            } else {
                compile_expr(expr, unit);
                let opcode = match op.as_str() {
                    "-" => Op::Neg,
                    "+" => Op::UnaryPlus,
                    "!" => Op::Not,
                    "~" => Op::BitNot,
                    _ => Op::Nop,
                };
                if opcode != Op::Nop {
                    unit.code.push(Instr::new(opcode, 0));
                }
            }
        }
        Expr::Binary { op, left, right } => match op.as_str() {
            "||" => {
                compile_expr(left, unit);
                let jmp_idx = emit_jump(unit, Op::JmpTrueKeep);
                compile_expr(right, unit);
                let end_idx = unit.code.len();
                backpatch_jump(unit, jmp_idx, end_idx);
            }
            "&&" => {
                compile_expr(left, unit);
                let jmp_idx = emit_jump(unit, Op::JmpFalseKeep);
                compile_expr(right, unit);
                let end_idx = unit.code.len();
                backpatch_jump(unit, jmp_idx, end_idx);
            }
            "??" => {
                compile_expr(left, unit);
                let jmp_idx = emit_jump(unit, Op::JmpNullishKeep);
                compile_expr(right, unit);
                let end_idx = unit.code.len();
                backpatch_jump(unit, jmp_idx, end_idx);
            }
            _ => {
                compile_expr(left, unit);
                compile_expr(right, unit);
                let opcode = match op.as_str() {
                    "+" => Op::Add,
                    "-" => Op::Sub,
                    "*" => Op::Mul,
                    "/" => Op::Div,
                    "%" => Op::Mod,
                    "**" => Op::Pow,
                    "==" => Op::Eq,
                    "!=" => Op::Ne,
                    "===" => Op::StrictEq,
                    "!==" => Op::StrictNe,
                    "<" => Op::Lt,
                    "<=" => Op::Le,
                    ">" => Op::Gt,
                    ">=" => Op::Ge,
                    "instanceof" => Op::Instanceof,
                    "in" => Op::In,
                    "&" => Op::BitAnd,
                    "|" => Op::BitOr,
                    "^" => Op::BitXor,
                    "<<" => Op::Shl,
                    ">>" => Op::Shr,
                    ">>>" => Op::UShr,
                    _ => Op::Add,
                };
                unit.code.push(Instr::new(opcode, 0));
            }
        },
        Expr::Update { op, target, prefix } => {
            let update_op = if op == "++" { Op::Inc } else { Op::Dec };
            if let Expr::Ident(name) = target.as_ref() {
                if let Some(&slot) = unit.symbol_map.get(name) {
                    unit.code.push(Instr::new(Op::LoadLocal, slot as u32));
                    if *prefix {
                        unit.code.push(Instr::new(update_op, 0));
                        unit.code.push(Instr::new(Op::Dup, 0));
                        unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                    } else {
                        unit.code.push(Instr::new(Op::Dup, 0));
                        unit.code.push(Instr::new(update_op, 0));
                        unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                    }
                } else if let Some(&uv_idx) = unit.upvalue_map.get(name) {
                    unit.code.push(Instr::new(Op::LoadUpvalue, uv_idx as u32));
                    if *prefix {
                        unit.code.push(Instr::new(update_op, 0));
                        unit.code.push(Instr::new(Op::Dup, 0));
                        unit.code.push(Instr::new(Op::StoreUpvalue, uv_idx as u32));
                    } else {
                        unit.code.push(Instr::new(Op::Dup, 0));
                        unit.code.push(Instr::new(update_op, 0));
                        unit.code.push(Instr::new(Op::StoreUpvalue, uv_idx as u32));
                    }
                } else if unit.implicit_globals {
                    // 隐式全局模式（eval）：未声明/全局绑定经 LoadGlobal/StoreGlobal
                    let name_idx = add_constant(unit, Constant::String(name.clone()));
                    unit.code.push(Instr::new(Op::LoadGlobal, name_idx));
                    if *prefix {
                        unit.code.push(Instr::new(update_op, 0));
                        unit.code.push(Instr::new(Op::Dup, 0));
                        unit.code.push(Instr::new(Op::StoreGlobal, name_idx));
                    } else {
                        unit.code.push(Instr::new(Op::Dup, 0));
                        unit.code.push(Instr::new(update_op, 0));
                        unit.code.push(Instr::new(Op::StoreGlobal, name_idx));
                    }
                } else {
                    let slot = unit.locals;
                    unit.locals += 1;
                    unit.symbol_map.insert(name.clone(), slot);
                    unit.code.push(Instr::new(Op::PushInt, 0));
                    unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                    unit.code.push(Instr::new(Op::LoadLocal, slot as u32));
                    if *prefix {
                        unit.code.push(Instr::new(update_op, 0));
                        unit.code.push(Instr::new(Op::Dup, 0));
                        unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                    } else {
                        unit.code.push(Instr::new(Op::Dup, 0));
                        unit.code.push(Instr::new(update_op, 0));
                        unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                    }
                }
            } else {
                unit.code.push(Instr::new(Op::PushUndefined, 0));
            }
        }
        Expr::Conditional {
            cond,
            then_expr,
            else_expr,
        } => {
            compile_expr(cond, unit);
            let jmp_false_idx = emit_jump(unit, Op::JmpFalsePop);
            compile_expr(then_expr, unit);
            let jmp_end_idx = emit_jump(unit, Op::Jmp);
            let else_start = unit.code.len();
            backpatch_jump(unit, jmp_false_idx, else_start);
            compile_expr(else_expr, unit);
            let end_idx = unit.code.len();
            backpatch_jump(unit, jmp_end_idx, end_idx);
        }
        Expr::Object(props) => {
            unit.code.push(Instr::new(Op::NewObject, 0));
            for prop in props {
                match (&prop.key, &prop.value) {
                    (PropKey::Literal(k), PropValue::Expr(v)) => {
                        compile_expr(v, unit);
                        let name_idx = add_constant(unit, Constant::String(k.clone()));
                        unit.code.push(Instr::new(Op::SetPropObj, name_idx));
                    }
                    (PropKey::Computed(k), PropValue::Expr(v)) => {
                        compile_expr(k, unit);
                        compile_expr(v, unit);
                        unit.code.push(Instr::new(Op::SetPropComputedObj, 0));
                    }
                    (PropKey::Literal(k), PropValue::Getter(def)) => {
                        let instr_idx = unit.code.len();
                        unit.code.push(Instr::new(Op::MakeClosure, 0));
                        unit.closure_backpatches.push((
                            instr_idx,
                            def.clone(),
                            ParentScopeInfo::new(unit.symbol_map.clone(), unit.upvalue_map.clone()),
                        ));
                        let name_idx = add_constant(unit, Constant::String(k.clone()));
                        unit.code.push(Instr::new(Op::SetGetterObj, name_idx));
                    }
                    (PropKey::Literal(k), PropValue::Setter(def)) => {
                        let instr_idx = unit.code.len();
                        unit.code.push(Instr::new(Op::MakeClosure, 0));
                        unit.closure_backpatches.push((
                            instr_idx,
                            def.clone(),
                            ParentScopeInfo::new(unit.symbol_map.clone(), unit.upvalue_map.clone()),
                        ));
                        let name_idx = add_constant(unit, Constant::String(k.clone()));
                        unit.code.push(Instr::new(Op::SetSetterObj, name_idx));
                    }
                    (PropKey::Computed(k), PropValue::Getter(def)) => {
                        compile_expr(k, unit);
                        let instr_idx = unit.code.len();
                        unit.code.push(Instr::new(Op::MakeClosure, 0));
                        unit.closure_backpatches.push((
                            instr_idx,
                            def.clone(),
                            ParentScopeInfo::new(unit.symbol_map.clone(), unit.upvalue_map.clone()),
                        ));
                        unit.code.push(Instr::new(Op::SetGetterComputedObj, 0));
                    }
                    (PropKey::Computed(k), PropValue::Setter(def)) => {
                        compile_expr(k, unit);
                        let instr_idx = unit.code.len();
                        unit.code.push(Instr::new(Op::MakeClosure, 0));
                        unit.closure_backpatches.push((
                            instr_idx,
                            def.clone(),
                            ParentScopeInfo::new(unit.symbol_map.clone(), unit.upvalue_map.clone()),
                        ));
                        unit.code.push(Instr::new(Op::SetSetterComputedObj, 0));
                    }
                    (_, PropValue::Spread(inner)) => {
                        compile_expr(inner, unit);
                        unit.code.push(Instr::new(Op::SpreadObject, 0));
                    }
                }
            }
        }
        Expr::Array(elems) => {
            let has_spread = elems.iter().any(|e| matches!(e, Expr::Spread(_)));
            if !has_spread {
                for elem in elems {
                    compile_expr(elem, unit);
                }
                unit.code.push(Instr::new(Op::NewArray, elems.len() as u32));
            } else {
                unit.code.push(Instr::new(Op::BuildArray, 0));
                for elem in elems {
                    match elem {
                        Expr::Spread(inner) => {
                            compile_expr(inner, unit);
                            unit.code.push(Instr::new(Op::ArraySpread, 0));
                        }
                        _ => {
                            compile_expr(elem, unit);
                            unit.code.push(Instr::new(Op::ArrayPush, 0));
                        }
                    }
                }
            }
        }
        Expr::Member { obj, prop } => {
            if matches!(obj.as_ref(), Expr::Super) {
                if let Some(cid) = unit.class_id {
                    let proto_name = format!("__home_proto_{cid}__");
                    if let Some(&slot) = unit.symbol_map.get(&proto_name) {
                        unit.code.push(Instr::new(Op::LoadLocal, slot as u32));
                    } else if let Some(&uv_idx) = unit.upvalue_map.get(&proto_name) {
                        unit.code.push(Instr::new(Op::LoadUpvalue, uv_idx as u32));
                    } else {
                        unit.code.push(Instr::new(Op::PushUndefined, 0));
                    }
                } else {
                    unit.code.push(Instr::new(Op::PushUndefined, 0));
                }
            } else {
                compile_expr(obj, unit);
            }
            let p_idx = add_constant(unit, Constant::String(prop.clone()));
            unit.code.push(Instr::new(Op::GetProp, p_idx));
        }
        Expr::Index { obj, index } => {
            compile_expr(obj, unit);
            compile_expr(index, unit);
            unit.code.push(Instr::new(Op::GetElem, 0));
        }
        Expr::MemberAssign { obj, prop, value } => {
            compile_expr(obj, unit);
            compile_expr(value, unit);
            let p_idx = add_constant(unit, Constant::String(prop.clone()));
            unit.code.push(Instr::new(Op::SetProp, p_idx));
        }
        Expr::IndexAssign { obj, index, value } => {
            compile_expr(obj, unit);
            compile_expr(index, unit);
            compile_expr(value, unit);
            unit.code.push(Instr::new(Op::SetElem, 0));
        }
        Expr::Call { callee, args } => {
            let has_spread = args.iter().any(|a| matches!(a, Expr::Spread(_)));
            if matches!(callee.as_ref(), Expr::Super) {
                if let Some(cid) = unit.class_id {
                    let ctor_name = format!("__home_ctor_{cid}__");
                    if let Some(&slot) = unit.symbol_map.get(&ctor_name) {
                        unit.code.push(Instr::new(Op::LoadLocal, slot as u32));
                    } else if let Some(&uv_idx) = unit.upvalue_map.get(&ctor_name) {
                        unit.code.push(Instr::new(Op::LoadUpvalue, uv_idx as u32));
                    } else {
                        unit.code.push(Instr::new(Op::PushUndefined, 0));
                    }
                } else {
                    unit.code.push(Instr::new(Op::PushUndefined, 0));
                }
                if !has_spread {
                    for arg in args {
                        compile_expr(arg, unit);
                    }
                    unit.code
                        .push(Instr::new(Op::ConstructThis, args.len() as u32));
                } else {
                    compile_args_array(args, unit);
                    unit.code.push(Instr::new(Op::ConstructThisArgs, 0));
                }
            } else if let Expr::Index { obj, index } = callee.as_ref() {
                // 计算成员方法调用：obj[index](args) -> 保持 this 绑定
                compile_expr(obj, unit);
                unit.code.push(Instr::new(Op::Dup, 0));
                compile_expr(index, unit);
                unit.code.push(Instr::new(Op::GetElem, 0));
                unit.code.push(Instr::new(Op::Swap, 0));
                if !has_spread {
                    for arg in args {
                        compile_expr(arg, unit);
                    }
                    unit.code
                        .push(Instr::new(Op::CallWithThis, args.len() as u32));
                } else {
                    compile_args_array(args, unit);
                    unit.code.push(Instr::new(Op::CallWithThisArgs, 0));
                }
            } else {
                // 直接求值标记：裸标识符 `eval(...)` 的调用形态改经专管
                // 全局名分派（运行时区分直接/间接求值），并给当前函数
                // 单元打 has_direct_eval 降级标记
                let is_direct_eval = matches!(callee.as_ref(), Expr::Ident(id) if id == "eval");
                if is_direct_eval {
                    unit.has_direct_eval = true;
                    let idx =
                        add_constant(unit, Constant::String("%aluka_direct_eval%".to_owned()));
                    unit.code.push(Instr::new(Op::LoadGlobal, idx));
                } else {
                    compile_expr(callee, unit);
                }
                if !has_spread {
                    for arg in args {
                        compile_expr(arg, unit);
                    }
                    unit.code.push(Instr::new(Op::Call, args.len() as u32));
                } else {
                    compile_args_array(args, unit);
                    unit.code.push(Instr::new(Op::CallArgs, 0));
                }
            }
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
        } => {
            let has_spread = args.iter().any(|a| matches!(a, Expr::Spread(_)));
            if matches!(receiver.as_ref(), Expr::Super) {
                if let Some(cid) = unit.class_id {
                    let proto_name = format!("__home_proto_{cid}__");
                    if let Some(&slot) = unit.symbol_map.get(&proto_name) {
                        unit.code.push(Instr::new(Op::LoadLocal, slot as u32));
                    } else if let Some(&uv_idx) = unit.upvalue_map.get(&proto_name) {
                        unit.code.push(Instr::new(Op::LoadUpvalue, uv_idx as u32));
                    } else {
                        unit.code.push(Instr::new(Op::PushUndefined, 0));
                    }
                } else {
                    unit.code.push(Instr::new(Op::PushUndefined, 0));
                }
                let name_idx = add_constant(unit, Constant::String(method.clone()));
                unit.code.push(Instr::new(Op::GetProp, name_idx));
                for arg in args {
                    compile_expr(arg, unit);
                }
                unit.code.push(Instr::new(Op::CallThis, args.len() as u32));
            } else {
                let name_idx = add_constant(unit, Constant::String(method.clone()));
                compile_expr(receiver, unit);
                if !has_spread {
                    for arg in args {
                        compile_expr(arg, unit);
                    }
                    let operand = ((args.len() as u32) << 16) | (name_idx & 0xFFFF);
                    unit.code.push(Instr::new(Op::CallMethod, operand));
                } else {
                    compile_args_array(args, unit);
                    unit.code.push(Instr::new(Op::CallMethodArgs, name_idx));
                }
            }
        }
        Expr::New { callee, args } => {
            compile_expr(callee, unit);
            for arg in args {
                compile_expr(arg, unit);
            }
            unit.code.push(Instr::new(Op::New, args.len() as u32));
        }
        Expr::OptionalMember { obj, prop } => {
            compile_expr(obj, unit);
            let opt_jmp_idx = emit_jump(unit, Op::OptionalJump);
            let p_idx = add_constant(unit, Constant::String(prop.clone()));
            unit.code.push(Instr::new(Op::GetProp, p_idx));
            let target_idx = unit.code.len();
            backpatch_jump(unit, opt_jmp_idx, target_idx);
        }
        Expr::OptionalIndex { obj, index } => {
            compile_expr(obj, unit);
            let opt_jmp_idx = emit_jump(unit, Op::OptionalJump);
            compile_expr(index, unit);
            unit.code.push(Instr::new(Op::GetElem, 0));
            let target_idx = unit.code.len();
            backpatch_jump(unit, opt_jmp_idx, target_idx);
        }
        Expr::OptionalCall { callee, args } => {
            compile_expr(callee, unit);
            let opt_jmp_idx = emit_jump(unit, Op::OptionalJump);
            for arg in args {
                compile_expr(arg, unit);
            }
            unit.code.push(Instr::new(Op::Call, args.len() as u32));
            let target_idx = unit.code.len();
            backpatch_jump(unit, opt_jmp_idx, target_idx);
        }
        Expr::Function(def) => {
            let instr_idx = unit.code.len();
            unit.code.push(Instr::new(Op::MakeClosure, 0));
            unit.closure_backpatches.push((
                instr_idx,
                def.clone(),
                ParentScopeInfo::new(unit.symbol_map.clone(), unit.upvalue_map.clone()),
            ));
        }
        Expr::Spread(inner) => {
            compile_expr(inner, unit);
        }
        Expr::RegExp { pattern, flags } => {
            let pat_idx = add_constant(unit, Constant::String(pattern.clone()));
            let flags_idx = add_constant(unit, Constant::String(flags.clone()));
            unit.code.push(Instr::new(Op::PushConst, pat_idx));
            unit.code.push(Instr::new(Op::PushConst, flags_idx));
            unit.code.push(Instr::new(Op::MakeRegexp, 0));
        }
        Expr::Super => {
            unit.code.push(Instr::new(Op::PushUndefined, 0));
        }
        Expr::Yield { value, delegate } => {
            if *delegate {
                compile_expr(value.as_ref().unwrap(), unit);
                unit.code.push(Instr::new(Op::GetIterator, 0));
                let tmp_iter = unit.locals;
                unit.locals += 1;
                unit.code.push(Instr::new(Op::StoreLocal, tmp_iter as u32));

                let tmp_result = unit.locals;
                unit.locals += 1;

                let name_next = add_constant(unit, Constant::String("next".to_owned()));
                let name_done = add_constant(unit, Constant::String("done".to_owned()));
                let name_value = add_constant(unit, Constant::String("value".to_owned()));

                let loop_start = unit.code.len();
                unit.code.push(Instr::new(Op::LoadLocal, tmp_iter as u32));
                unit.code.push(Instr::new(Op::CallMethod, name_next));
                unit.code
                    .push(Instr::new(Op::StoreLocal, tmp_result as u32));

                unit.code.push(Instr::new(Op::LoadLocal, tmp_result as u32));
                unit.code.push(Instr::new(Op::GetProp, name_done));
                let exit_jmp = emit_jump(unit, Op::JmpTruePop);

                unit.code.push(Instr::new(Op::LoadLocal, tmp_result as u32));
                unit.code.push(Instr::new(Op::GetProp, name_value));
                unit.code.push(Instr::new(Op::Yield, 0));
                unit.code.push(Instr::new(Op::Pop, 0));

                let loop_back = emit_jump(unit, Op::Jmp);
                backpatch_jump(unit, loop_back, loop_start);

                let loop_end = unit.code.len();
                backpatch_jump(unit, exit_jmp, loop_end);

                unit.code.push(Instr::new(Op::LoadLocal, tmp_result as u32));
                unit.code.push(Instr::new(Op::GetProp, name_value));
            } else {
                if let Some(arg) = value {
                    compile_expr(arg, unit);
                } else {
                    unit.code.push(Instr::new(Op::PushUndefined, 0));
                }
                unit.code.push(Instr::new(Op::Yield, 0));
            }
        }
        Expr::Await(arg) => {
            compile_expr(arg, unit);
            unit.code.push(Instr::new(Op::Await, 0));
        }
        Expr::TemplateLiteral { quasis, exprs } => {
            compile_template_literal(quasis, exprs, unit);
        }
        Expr::Sequence { exprs } => {
            // 依次求值，仅保留末位值（栈上弹出其余）
            let n = exprs.len();
            for (i, e) in exprs.iter().enumerate() {
                compile_expr(e, unit);
                if i + 1 < n {
                    unit.code.push(Instr::new(Op::Pop, 0));
                }
            }
            if n == 0 {
                unit.code.push(Instr::new(Op::PushUndefined, 0));
            }
        }
        Expr::TaggedTemplate {
            tag,
            quasis,
            raws,
            exprs,
        } => {
            // 调用序列：tag(strings, ...插值)。strings 为 cooked 片段数组，
            // 带 `raw` 属性（原始片段数组）——模板字符串数组对象的 JS 表面。
            // 成员 tag（obj.tag`x`）按方法调用形态编译：this 绑定接收者。
            let member = match tag.as_ref() {
                Expr::Member { obj, prop } => Some((obj.as_ref().clone(), prop.clone())),
                _ => None,
            };
            if let Some((recv, prop)) = member {
                compile_expr(&recv, unit);
                let name_idx = add_constant(unit, Constant::String(prop));
                emit_tagged_args(quasis, raws, exprs, unit);
                let operand = (((1 + exprs.len()) as u32) << 16) | (name_idx & 0xFFFF);
                unit.code.push(Instr::new(Op::CallMethod, operand));
            } else {
                compile_expr(tag, unit);
                emit_tagged_args(quasis, raws, exprs, unit);
                unit.code
                    .push(Instr::new(Op::Call, (1 + exprs.len()) as u32));
            }
        }
    }
}

/// 静态分析：检查语句及其子树中的闭包是否引用了指定的局部变量名
fn stmt_has_closure_capturing(s: &SpannedStmt, target_name: &str) -> bool {
    let stmt = &s.stmt;
    match stmt {
        Stmt::Expr(expr) => expr_has_closure_capturing(expr, target_name),
        Stmt::VarDecl {
            init: Some(init), ..
        } => expr_has_closure_capturing(init, target_name),
        Stmt::VarDecl { init: None, .. } => false,
        Stmt::MultiVarDecl { decls, .. } => decls.iter().any(|(_, init)| {
            init.as_ref()
                .is_some_and(|e| expr_has_closure_capturing(e, target_name))
        }),
        Stmt::DestructureDecl { init, .. } => expr_has_closure_capturing(init, target_name),
        Stmt::Block(stmts) => stmts
            .iter()
            .any(|s| stmt_has_closure_capturing(s, target_name)),
        Stmt::If {
            cond,
            then_branch,
            else_branch,
        } => {
            expr_has_closure_capturing(cond, target_name)
                || stmt_has_closure_capturing(then_branch, target_name)
                || else_branch
                    .as_ref()
                    .is_some_and(|b| stmt_has_closure_capturing(b, target_name))
        }
        Stmt::While { cond, body } | Stmt::DoWhile { cond, body } => {
            expr_has_closure_capturing(cond, target_name)
                || stmt_has_closure_capturing(body, target_name)
        }
        Stmt::Return(Some(expr)) => expr_has_closure_capturing(expr, target_name),
        Stmt::Throw(expr) => expr_has_closure_capturing(expr, target_name),
        Stmt::Return(None) | Stmt::Break | Stmt::Continue => false,
        Stmt::For {
            init,
            cond,
            update,
            body,
        } => {
            init.as_ref()
                .is_some_and(|s| stmt_has_closure_capturing(s, target_name))
                || cond
                    .as_ref()
                    .is_some_and(|e| expr_has_closure_capturing(e, target_name))
                || update
                    .as_ref()
                    .is_some_and(|e| expr_has_closure_capturing(e, target_name))
                || stmt_has_closure_capturing(body, target_name)
        }
        Stmt::Try {
            body,
            catch_body,
            finally_body,
            ..
        } => {
            stmt_has_closure_capturing(body, target_name)
                || catch_body
                    .as_ref()
                    .is_some_and(|s| stmt_has_closure_capturing(s, target_name))
                || finally_body
                    .as_ref()
                    .is_some_and(|s| stmt_has_closure_capturing(s, target_name))
        }
        Stmt::Function(def) => {
            let mut uses = Vec::new();
            for s in &def.body {
                collect_ident_uses(s, &mut uses);
            }
            uses.iter().any(|u| u == target_name)
        }
        Stmt::Switch {
            discriminant,
            cases,
        } => {
            expr_has_closure_capturing(discriminant, target_name)
                || cases.iter().any(|c| {
                    c.test
                        .as_ref()
                        .is_some_and(|t| expr_has_closure_capturing(t, target_name))
                        || c.consequent
                            .iter()
                            .any(|s| stmt_has_closure_capturing(s, target_name))
                })
        }
        Stmt::Class { .. } => false,
        Stmt::ForIn { right, body, .. } | Stmt::ForOf { right, body, .. } => {
            expr_has_closure_capturing(right, target_name)
                || stmt_has_closure_capturing(body, target_name)
        }
        Stmt::Import(_) => false,
        Stmt::Export(export_decl) => match export_decl {
            aluka_parser::ast::ExportDecl::Named {
                decl: Some(inner), ..
            } => stmt_has_closure_capturing(inner, target_name),
            aluka_parser::ast::ExportDecl::Default(expr) => {
                expr_has_closure_capturing(expr, target_name)
            }
            _ => false,
        },
    }
}

/// 静态分析：检查表达式及其子树中的闭包是否引用了指定的局部变量名
fn expr_has_closure_capturing(expr: &Expr, target_name: &str) -> bool {
    match expr {
        Expr::Function(def) => {
            let mut uses = Vec::new();
            for s in &def.body {
                collect_ident_uses(s, &mut uses);
            }
            uses.iter().any(|u| u == target_name)
        }
        Expr::Unary { expr, .. } => expr_has_closure_capturing(expr, target_name),
        Expr::Binary { left, right, .. } => {
            expr_has_closure_capturing(left, target_name)
                || expr_has_closure_capturing(right, target_name)
        }
        Expr::Assign { value, .. } => expr_has_closure_capturing(value, target_name),
        Expr::Update { target, .. } => expr_has_closure_capturing(target, target_name),
        Expr::Conditional {
            cond,
            then_expr,
            else_expr,
        } => {
            expr_has_closure_capturing(cond, target_name)
                || expr_has_closure_capturing(then_expr, target_name)
                || expr_has_closure_capturing(else_expr, target_name)
        }
        Expr::Call { callee, args }
        | Expr::New { callee, args }
        | Expr::OptionalCall { callee, args } => {
            expr_has_closure_capturing(callee, target_name)
                || args
                    .iter()
                    .any(|a| expr_has_closure_capturing(a, target_name))
        }
        Expr::MethodCall { receiver, args, .. } => {
            expr_has_closure_capturing(receiver, target_name)
                || args
                    .iter()
                    .any(|a| expr_has_closure_capturing(a, target_name))
        }
        Expr::Member { obj, .. } | Expr::OptionalMember { obj, .. } => {
            expr_has_closure_capturing(obj, target_name)
        }
        Expr::Index { obj, index } | Expr::OptionalIndex { obj, index } => {
            expr_has_closure_capturing(obj, target_name)
                || expr_has_closure_capturing(index, target_name)
        }
        Expr::Object(props) => props.iter().any(|p| {
            let key_captures = match &p.key {
                PropKey::Computed(k) => expr_has_closure_capturing(k, target_name),
                PropKey::Literal(_) => false,
            };
            let val_captures = match &p.value {
                PropValue::Expr(v) | PropValue::Spread(v) => {
                    expr_has_closure_capturing(v, target_name)
                }
                PropValue::Getter(def) | PropValue::Setter(def) => {
                    let mut uses = Vec::new();
                    for s in &def.body {
                        collect_ident_uses(s, &mut uses);
                    }
                    uses.iter().any(|u| u == target_name)
                }
            };
            key_captures || val_captures
        }),
        Expr::Array(elements) => elements
            .iter()
            .any(|e| expr_has_closure_capturing(e, target_name)),
        Expr::Spread(inner) => expr_has_closure_capturing(inner, target_name),
        Expr::Yield { value: Some(v), .. } => expr_has_closure_capturing(v, target_name),
        Expr::Yield { value: None, .. } => false,
        Expr::Await(arg) => expr_has_closure_capturing(arg, target_name),
        Expr::TemplateLiteral { exprs, .. } => exprs
            .iter()
            .any(|e| expr_has_closure_capturing(e, target_name)),
        Expr::Super => false,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_number_literal_then_returns() {
        let program = Program {
            body: vec![SpannedStmt::new(Stmt::Expr(Expr::Number(7.0)), 0)],
        };
        let unit = compile(&program);
        assert_eq!(
            unit.code,
            vec![Instr::new(Op::PushInt, 7), Instr::new(Op::Return, 0)]
        );
    }

    #[test]
    fn compiles_addition_in_evaluation_order() {
        let program = Program {
            body: vec![SpannedStmt::new(
                Stmt::Expr(Expr::Binary {
                    op: "+".to_owned(),
                    left: Box::new(Expr::Number(1.0)),
                    right: Box::new(Expr::Number(2.0)),
                }),
                0,
            )],
        };
        let unit = compile(&program);
        assert_eq!(
            unit.code,
            vec![
                Instr::new(Op::PushInt, 1),
                Instr::new(Op::PushInt, 2),
                Instr::new(Op::Add, 0),
                Instr::new(Op::Return, 0),
            ]
        );
    }

    #[test]
    fn oversized_literal_falls_back_off_the_immediate_path() {
        let big = f64::from(u32::MAX);
        let program = Program {
            body: vec![SpannedStmt::new(Stmt::Expr(Expr::Number(big)), 0)],
        };
        let unit = compile(&program);
        assert_eq!(unit.code[0], Instr::new(Op::PushConst, 0));
        assert_eq!(unit.constants, vec![Constant::Number(big)]);
    }
}
