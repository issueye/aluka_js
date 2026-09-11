//! 模块级多函数与类模板汇编器。

use crate::codegen::{compile_expr, compile_stmt};
use crate::scope::{CompiledUnit, ParentScopeInfo};
use aluka_bytecode::{
    BytecodeModule, ClassMethod, ClassTemplate, Constant, FuncHeaderExtras, FuncTemplate, Instr,
    Op, UpvalueCapture,
};
use aluka_parser::ast::{
    ClassMethodDef, Expr, FunctionDef, Program, PropKey, PropValue, Stmt, VarKind, VarPattern,
};

/// 编译整个 AST 语法树模块，生成包含函数模板与类模板的完整字节码模块。
#[must_use]
pub fn compile_module(program: &Program) -> BytecodeModule {
    let mut compiler = ModuleCompiler::new();
    compiler.compile(program)
}

/// 以 ESM 包装形态编译（CJS 7 参闭包 + exports 绑定），供 `require(esm)` 互操作。
#[must_use]
pub fn compile_esm_module(program: &Program) -> BytecodeModule {
    let mut compiler = ModuleCompiler::new();
    compiler.is_esm = true;
    compiler.compile(program)
}

/// 模块级编译上下文。
#[derive(Debug, Default)]
pub struct ModuleCompiler {
    /// 函数模板池
    pub functions: Vec<FuncTemplate>,
    /// 类模板池
    pub classes: Vec<ClassTemplate>,
    /// 函数扩展标量头（与 `functions` 平行；main 为默认「无 arguments」）
    pub header_extras: Vec<FuncHeaderExtras>,
    /// 以 ESM 包装形态编译（CJS 7 参闭包 + exports 绑定）
    pub is_esm: bool,
    /// 保留脚本完成值（eval 动态求值用）：main 以 `Return` 收口返回末语句值，
    /// 而非恒 `ReturnUndef`。默认 false（require/入口模块语义不变）。
    pub preserve_completion_value: bool,
    /// 隐式全局模式（eval 全局作用域求值用）：转发至顶层编译单元。
    pub implicit_globals: bool,
    /// ESM import 声明计数（合成命名空间绑定名的唯一性）
    pub esm_import_counter: usize,
}

fn collect_ident_uses_in_expr(expr: &Expr, uses: &mut Vec<String>) {
    match expr {
        Expr::Ident(name) => uses.push(name.clone()),
        Expr::Assign { name, value } => {
            uses.push(name.clone());
            collect_ident_uses_in_expr(value, uses);
        }
        Expr::Binary { left, right, .. } => {
            collect_ident_uses_in_expr(left, uses);
            collect_ident_uses_in_expr(right, uses);
        }
        Expr::Unary { expr, .. } => collect_ident_uses_in_expr(expr, uses),
        Expr::Call { callee, args } | Expr::New { callee, args } => {
            collect_ident_uses_in_expr(callee, uses);
            for a in args {
                collect_ident_uses_in_expr(a, uses);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            collect_ident_uses_in_expr(receiver, uses);
            for a in args {
                collect_ident_uses_in_expr(a, uses);
            }
        }
        Expr::Member { obj, .. } | Expr::OptionalMember { obj, .. } => {
            collect_ident_uses_in_expr(obj, uses);
        }
        // 属性/下标赋值：目标对象与下标都是标识符使用点（遗漏会把
        // 闭包捕获退化为 LoadGlobal，赋值静默落到错误的作用域）
        Expr::MemberAssign { obj, value, .. } => {
            collect_ident_uses_in_expr(obj, uses);
            collect_ident_uses_in_expr(value, uses);
        }
        Expr::IndexAssign { obj, index, value } => {
            collect_ident_uses_in_expr(obj, uses);
            collect_ident_uses_in_expr(index, uses);
            collect_ident_uses_in_expr(value, uses);
        }
        Expr::Index { obj, index } | Expr::OptionalIndex { obj, index } => {
            collect_ident_uses_in_expr(obj, uses);
            collect_ident_uses_in_expr(index, uses);
        }
        Expr::Object(props) => {
            for p in props {
                if let PropKey::Computed(k) = &p.key {
                    collect_ident_uses_in_expr(k, uses);
                }
                match &p.value {
                    PropValue::Expr(v) | PropValue::Spread(v) => {
                        collect_ident_uses_in_expr(v, uses)
                    }
                    PropValue::Getter(def) | PropValue::Setter(def) => {
                        for s in &def.body {
                            collect_ident_uses(s, uses);
                        }
                    }
                }
            }
        }
        Expr::Array(elements) => {
            for e in elements {
                collect_ident_uses_in_expr(e, uses);
            }
        }
        Expr::Spread(inner) => {
            collect_ident_uses_in_expr(inner, uses);
        }
        Expr::Update { target, .. } => collect_ident_uses_in_expr(target, uses),
        Expr::Conditional {
            cond,
            then_expr,
            else_expr,
        } => {
            collect_ident_uses_in_expr(cond, uses);
            collect_ident_uses_in_expr(then_expr, uses);
            collect_ident_uses_in_expr(else_expr, uses);
        }
        Expr::OptionalCall { callee, args } => {
            collect_ident_uses_in_expr(callee, uses);
            for a in args {
                collect_ident_uses_in_expr(a, uses);
            }
        }
        Expr::Function(def) => {
            for stmt in &def.body {
                collect_ident_uses(stmt, uses);
            }
        }
        Expr::Yield { value: Some(v), .. } => collect_ident_uses_in_expr(v, uses),
        Expr::Yield { value: None, .. } => {}
        Expr::Await(arg) => collect_ident_uses_in_expr(arg, uses),
        Expr::TemplateLiteral { exprs, .. } => {
            for e in exprs {
                collect_ident_uses_in_expr(e, uses);
            }
        }
        Expr::Super => {}
        _ => {}
    }
}

/// 顶层语句是否声明了名为 `name` 的绑定（`arguments` 遮蔽检测用）。
///
/// 只检查函数体顶层声明；块级 `let arguments` 的遮蔽留待更细粒度作用域分析，
/// 当前 Go 前端同样不做块级遮蔽（保守地分配 arguments 槽）。
fn stmt_declares(stmt: &Stmt, name: &str) -> bool {
    match stmt {
        Stmt::VarDecl { name: n, .. } => n == name,
        Stmt::Function(def) => def.name == name,
        _ => false,
    }
}

pub(crate) fn collect_ident_uses(stmt: &Stmt, uses: &mut Vec<String>) {
    match stmt {
        Stmt::Expr(expr) => collect_ident_uses_in_expr(expr, uses),
        // 嵌套函数声明体递归收集（与函数表达式/访问器路径对齐）：孙级函数
        // 对外层名的引用需求必须上抛到父函数捕获集——否则父函数自身不引用
        // 该名时中间层 upvalue 链断裂，孙函数引用退化为 LoadGlobal
        // （raw-body readStream → done → invokeCallback 的 cleanup 实测）
        Stmt::Function(def) => {
            for s in &def.body {
                collect_ident_uses(s, uses);
            }
        }
        Stmt::VarDecl {
            init: Some(init), ..
        } => {
            collect_ident_uses_in_expr(init, uses);
        }
        Stmt::VarDecl { init: None, .. } => {}
        Stmt::MultiVarDecl { decls, .. } => {
            for (_, init) in decls {
                if let Some(e) = init {
                    collect_ident_uses_in_expr(e, uses);
                }
            }
        }
        Stmt::DestructureDecl { init, .. } => {
            collect_ident_uses_in_expr(init, uses);
        }
        Stmt::Block(stmts) => {
            for s in stmts {
                collect_ident_uses(s, uses);
            }
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_ident_uses_in_expr(cond, uses);
            collect_ident_uses(then_branch, uses);
            if let Some(eb) = else_branch {
                collect_ident_uses(eb, uses);
            }
        }
        Stmt::While { cond, body } | Stmt::DoWhile { cond, body } => {
            collect_ident_uses_in_expr(cond, uses);
            collect_ident_uses(body, uses);
        }
        Stmt::For {
            init,
            cond,
            update,
            body,
        } => {
            if let Some(i) = init {
                collect_ident_uses(i, uses);
            }
            if let Some(c) = cond {
                collect_ident_uses_in_expr(c, uses);
            }
            if let Some(u) = update {
                collect_ident_uses_in_expr(u, uses);
            }
            collect_ident_uses(body, uses);
        }
        Stmt::ForIn { right, body, .. } | Stmt::ForOf { right, body, .. } => {
            collect_ident_uses_in_expr(right, uses);
            collect_ident_uses(body, uses);
        }
        Stmt::Break | Stmt::Continue => {}
        Stmt::Return(Some(expr)) => collect_ident_uses_in_expr(expr, uses),
        Stmt::Try {
            body,
            catch_body,
            finally_body,
            ..
        } => {
            collect_ident_uses(body, uses);
            if let Some(cb) = catch_body {
                collect_ident_uses(cb, uses);
            }
            if let Some(fb) = finally_body {
                collect_ident_uses(fb, uses);
            }
        }
        Stmt::Throw(expr) => {
            collect_ident_uses_in_expr(expr, uses);
        }
        Stmt::Switch {
            discriminant,
            cases,
        } => {
            collect_ident_uses_in_expr(discriminant, uses);
            for c in cases {
                if let Some(t) = &c.test {
                    collect_ident_uses_in_expr(t, uses);
                }
                for s in &c.consequent {
                    collect_ident_uses(s, uses);
                }
            }
        }
        Stmt::Import(_) => {}
        Stmt::Export(export_decl) => match export_decl {
            aluka_parser::ast::ExportDecl::Named {
                decl: Some(inner), ..
            } => {
                collect_ident_uses(inner, uses);
            }
            aluka_parser::ast::ExportDecl::Default(expr) => {
                collect_ident_uses_in_expr(expr, uses);
            }
            _ => {}
        },
        _ => {}
    }
}

impl ModuleCompiler {
    /// 创建新的模块编译器实例。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 编译整个模块，返回符合规范的 BytecodeModule。
    pub fn compile(&mut self, program: &Program) -> BytecodeModule {
        let mut optimized_program = program.clone();
        crate::opt::optimize_ast(&mut optimized_program);

        if self.is_esm {
            return self.compile_esm(&optimized_program);
        }

        self.functions.clear();
        self.classes.clear();
        self.header_extras.clear();
        self.functions.push(FuncTemplate {
            name: "main".to_owned(),
            num_params: 0,
            num_locals: 0,
            is_var_args: false,
            is_generator: false,
            is_async: false,
            is_arrow: false,
            code: Vec::new(),
            max_stack: 0,
            source_file: String::new(),
            constants: Vec::new(),
            upvalues: Vec::new(),
            try_table: Vec::new(),
        });

        let mut top_unit = CompiledUnit {
            implicit_globals: self.implicit_globals,
            ..Default::default()
        };

        // 顶层函数声明提升（JS hoisting）：function 声明的闭包绑定必须
        // 先于其余语句求值（真实包在声明位置之前引用函数）。
        // 提升编译前先**预注册**全部顶层绑定名（var/解构/function/类），
        // 使提升函数的 ParentScopeInfo 快照包含完整符号表——否则后续
        // 声明的变量的 upvalue 捕获会静默丢失。
        // 递归收集（含块内声明）：块内 `function` 声明同样提升到本作用域
        // （Node 实测：块内可调用、块外可访问），故收集必须进入块语句。
        let mut hoisted: Vec<&aluka_parser::ast::Stmt> = Vec::new();
        let mut ordered: Vec<&aluka_parser::ast::Stmt> = Vec::new();
        collect_scope_functions(&optimized_program.body, &mut hoisted);
        for stmt in optimized_program.body.iter() {
            if !matches!(stmt, Stmt::Function(_)) {
                ordered.push(stmt);
            }
        }
        for stmt in optimized_program.body.iter() {
            // 隐式全局模式（eval）：var/function 顶层绑定落全局表，
            // 不预注册局部槽位（let/const/解构保持局部语义）
            if top_unit.implicit_globals {
                match stmt {
                    Stmt::Function(func_def) => {
                        let s = top_unit.locals;
                        top_unit.locals += 1;
                        top_unit.symbol_map.insert(func_def.name.clone(), s);
                    }
                    Stmt::VarDecl { name, kind, .. } if *kind == VarKind::Var => {}
                    Stmt::MultiVarDecl { decls, kind, .. } if *kind == VarKind::Var => {}
                    Stmt::VarDecl { name, .. } => {
                        ensure_slot(&mut top_unit, name);
                    }
                    Stmt::MultiVarDecl { decls, .. } => {
                        for (name, _) in decls {
                            ensure_slot(&mut top_unit, name);
                        }
                    }
                    Stmt::DestructureDecl { pattern, .. } => {
                        ensure_pattern_slots(&mut top_unit, pattern);
                    }
                    _ => {}
                }
                continue;
            }
            match stmt {
                Stmt::Function(func_def) => {
                    ensure_slot(&mut top_unit, &func_def.name);
                }
                Stmt::VarDecl { name, .. } => {
                    ensure_slot(&mut top_unit, name);
                }
                Stmt::MultiVarDecl { decls, .. } => {
                    for (name, _) in decls {
                        ensure_slot(&mut top_unit, name);
                    }
                }
                Stmt::DestructureDecl { pattern, .. } => {
                    ensure_pattern_slots(&mut top_unit, pattern);
                }
                Stmt::Class { name, .. } => {
                    ensure_slot(&mut top_unit, name);
                }
                _ => {}
            }
        }
        // 块内函数声明（已随递归收集进入 `hoisted`）同样提升到本作用域：为其
        // 预注册槽位，保证下面提升编译时的 ParentScopeInfo 快照包含这些名字
        // （否则块内函数体内的 upvalue 捕获会静默丢失）。`ensure_slot` 幂等，
        // 顶层函数名会被重复注册一次，无副作用。
        for stmt in &hoisted {
            if let Stmt::Function(func_def) = stmt {
                ensure_slot(&mut top_unit, &func_def.name);
            }
        }
        for f in &hoisted {
            if let aluka_parser::ast::Stmt::Function(func_def) = f {
                let slot = if let Some(&s) = top_unit.symbol_map.get(&func_def.name) {
                    s
                } else {
                    let s = top_unit.locals;
                    top_unit.locals += 1;
                    top_unit.symbol_map.insert(func_def.name.clone(), s);
                    s
                };
                let parent_info =
                    ParentScopeInfo::new(top_unit.symbol_map.clone(), top_unit.upvalue_map.clone());
                let fn_idx = self.compile_function_with_parent(func_def, Some(&parent_info));
                top_unit
                    .code
                    .push(Instr::new(Op::MakeClosure, fn_idx as u32));
                if top_unit.implicit_globals {
                    let name_idx = crate::codegen::add_constant(
                        &mut top_unit,
                        Constant::String(func_def.name.clone()),
                    );
                    top_unit.code.push(Instr::new(Op::StoreGlobal, name_idx));
                } else {
                    top_unit.code.push(Instr::new(Op::Dup, 0));
                    top_unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                    // 名字写全局：嵌套函数跨层 LOAD_GLOBAL 可见（depd deprecate→log）
                    let name_idx = crate::codegen::add_constant(
                        &mut top_unit,
                        Constant::String(func_def.name.clone()),
                    );
                    top_unit.code.push(Instr::new(Op::StoreGlobal, name_idx));
                }
            }
        }

        let n = ordered.len();
        for (i, stmt) in ordered.iter().enumerate() {
            let is_last = i == n - 1;
            match stmt {
                Stmt::Function(func_def) => {
                    let slot = if let Some(&s) = top_unit.symbol_map.get(&func_def.name) {
                        s
                    } else {
                        let s = top_unit.locals;
                        top_unit.locals += 1;
                        top_unit.symbol_map.insert(func_def.name.clone(), s);
                        s
                    };
                    let parent_info = ParentScopeInfo::new(
                        top_unit.symbol_map.clone(),
                        top_unit.upvalue_map.clone(),
                    );
                    let fn_idx = self.compile_function_with_parent(func_def, Some(&parent_info));
                    top_unit
                        .code
                        .push(Instr::new(Op::MakeClosure, fn_idx as u32));
                    if top_unit.implicit_globals {
                        let name_idx = crate::codegen::add_constant(
                            &mut top_unit,
                            Constant::String(func_def.name.clone()),
                        );
                        top_unit.code.push(Instr::new(Op::StoreGlobal, name_idx));
                    } else {
                        top_unit.code.push(Instr::new(Op::Dup, 0));
                        top_unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                        // 名字写全局：嵌套函数跨层 LOAD_GLOBAL 可见
                        let name_idx = crate::codegen::add_constant(
                            &mut top_unit,
                            Constant::String(func_def.name.clone()),
                        );
                        top_unit.code.push(Instr::new(Op::StoreGlobal, name_idx));
                    }
                }
                Stmt::Class {
                    name,
                    super_class,
                    constructor,
                    methods,
                } => {
                    let class_id = self.classes.len();
                    if let Some(super_expr) = super_class {
                        compile_expr(super_expr, &mut top_unit);
                        top_unit.code.push(Instr::new(Op::Dup, 0));

                        let ctor_sym = format!("__home_ctor_{class_id}__");
                        let ctor_slot = top_unit.locals;
                        top_unit.locals += 1;
                        top_unit.symbol_map.insert(ctor_sym, ctor_slot);
                        top_unit
                            .code
                            .push(Instr::new(Op::StoreLocal, ctor_slot as u32));

                        top_unit
                            .code
                            .push(Instr::new(Op::LoadLocal, ctor_slot as u32));
                        let proto_idx = crate::codegen::add_constant(
                            &mut top_unit,
                            aluka_bytecode::Constant::String("prototype".to_owned()),
                        );
                        top_unit.code.push(Instr::new(Op::GetProp, proto_idx));
                        let proto_sym = format!("__home_proto_{class_id}__");
                        let proto_slot = top_unit.locals;
                        top_unit.locals += 1;
                        top_unit.symbol_map.insert(proto_sym, proto_slot);
                        top_unit
                            .code
                            .push(Instr::new(Op::StoreLocal, proto_slot as u32));
                    }

                    let parent_info = ParentScopeInfo::new(
                        top_unit.symbol_map.clone(),
                        top_unit.upvalue_map.clone(),
                    );
                    let class_idx = self.compile_class(
                        name,
                        super_class.is_some(),
                        constructor,
                        methods,
                        Some(&parent_info),
                        class_id,
                    );
                    top_unit
                        .code
                        .push(Instr::new(Op::MakeClass, class_idx as u32));
                    let slot = if let Some(&s) = top_unit.symbol_map.get(name) {
                        s
                    } else {
                        let s = top_unit.locals;
                        top_unit.locals += 1;
                        top_unit.symbol_map.insert(name.clone(), s);
                        s
                    };
                    top_unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                }
                Stmt::Import(_) => {
                    // 静态导入在单模块执行中无需生成运行时操作指令
                }
                Stmt::Export(export_decl) => match export_decl {
                    aluka_parser::ast::ExportDecl::Named {
                        decl: Some(inner), ..
                    } => match inner.as_ref() {
                        Stmt::Function(func_def) => {
                            let parent_info = ParentScopeInfo::new(
                                top_unit.symbol_map.clone(),
                                top_unit.upvalue_map.clone(),
                            );
                            let fn_idx =
                                self.compile_function_with_parent(func_def, Some(&parent_info));
                            let slot = if let Some(&s) = top_unit.symbol_map.get(&func_def.name) {
                                s
                            } else {
                                let s = top_unit.locals;
                                top_unit.locals += 1;
                                top_unit.symbol_map.insert(func_def.name.clone(), s);
                                s
                            };
                            top_unit
                                .code
                                .push(Instr::new(Op::MakeClosure, fn_idx as u32));
                            top_unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                        }
                        other_inner => {
                            compile_stmt(other_inner, &mut top_unit, is_last);
                        }
                    },
                    aluka_parser::ast::ExportDecl::Default(expr) => {
                        compile_expr(expr, &mut top_unit);
                        if is_last {
                            top_unit.code.push(Instr::new(Op::Return, 0));
                        } else {
                            top_unit.code.push(Instr::new(Op::Pop, 0));
                        }
                    }
                    _ => {}
                },
                other => {
                    compile_stmt(other, &mut top_unit, is_last);
                }
            }
        }

        // 递归回填顶层单元中所有的闭包表达式占位指令
        while let Some((instr_idx, closure_def, mut parent_info)) =
            top_unit.closure_backpatches.pop()
        {
            for (k, v) in &top_unit.symbol_map {
                parent_info.locals.entry(k.clone()).or_insert(*v);
            }
            for (k, v) in &top_unit.upvalue_map {
                parent_info.upvalues.entry(k.clone()).or_insert(*v);
            }
            let child_idx = self.compile_function_with_parent(&closure_def, Some(&parent_info));
            top_unit.code[instr_idx].operand = child_idx as u32;
        }

        if top_unit.code.is_empty()
            || !matches!(
                top_unit.code.last().map(|i| i.op),
                Some(Op::Return | Op::ReturnUndef)
            )
        {
            // eval 动态求值需要脚本完成值：末语句为表达式语句时以
            // `Return` 收口（值已在栈顶）；其余维持 `ReturnUndef`
            let ends_with_expr = self.preserve_completion_value
                && matches!(optimized_program.body.last(), Some(Stmt::Expr(_)));
            top_unit.code.push(Instr::new(
                if ends_with_expr {
                    Op::Return
                } else {
                    Op::ReturnUndef
                },
                0,
            ));
        }

        let had_direct_eval_top = top_unit.has_direct_eval;
        let top_symbol_map = top_unit.symbol_map.clone();
        let mut top_func = top_unit.to_func_template("main");
        // 槽 0 保留给 this：顶层 `this` 表达式编译为 LoadLocal 0，
        // 即使无其余局部也至少需要 1 个槽位（与 ESM 路径一致）
        top_func.num_locals = top_func.num_locals.max(1);
        if had_direct_eval_top {
            attach_direct_eval_marker(&mut top_func, &top_symbol_map);
        }
        self.functions[0] = top_func;
        // main 不经过 compile_method_function，补一条默认扩展标量头（无 arguments）
        self.header_extras.insert(
            0,
            FuncHeaderExtras {
                arguments_slot: -1,
                no_arguments_object: true,
                new_target_slot: -1,
                inlinable: false,
            },
        );
        // 与反序列化对称：全部为默认标量时还原为空（roundtrip 严格等价）
        if self.header_extras.iter().all(|e| {
            e.arguments_slot == -1
                && e.no_arguments_object
                && e.new_target_slot == -1
                && !e.inlinable
        }) {
            self.header_extras.clear();
        }

        BytecodeModule {
            header_extras: std::mem::take(&mut self.header_extras),
            version: 30,
            functions: std::mem::take(&mut self.functions),
            classes: std::mem::take(&mut self.classes),
        }
    }

    /// ESM 包装编译：`main` 返回 CJS 7 参包装闭包（wrapper），wrapper 体把
    /// `export` 绑定到 `exports` 参数并打 `__esModule` 标记——使 `require(esm)`
    /// 复用 CJS 模块加载流程（`load_cjs_module` 的 7 参调用）。
    fn compile_esm(&mut self, program: &Program) -> BytecodeModule {
        self.functions.clear();
        self.classes.clear();
        self.header_extras.clear();
        self.functions.push(FuncTemplate {
            name: "main".to_owned(),
            num_params: 0,
            num_locals: 1,
            is_var_args: false,
            is_generator: false,
            is_async: false,
            is_arrow: false,
            code: Vec::new(),
            max_stack: 8,
            source_file: String::new(),
            constants: Vec::new(),
            upvalues: Vec::new(),
            try_table: Vec::new(),
        });
        self.header_extras.push(FuncHeaderExtras {
            arguments_slot: -1,
            no_arguments_object: true,
            new_target_slot: -1,
            inlinable: false,
        });

        // wrapper：CJS 7 参闭包（require, module, exports, ...）
        // locals 从 1 起（槽 0 = this），参数依次占槽 1..=7，与 VM 参数绑定对齐
        let mut unit = CompiledUnit {
            locals: 1,
            num_params: 7,
            ..Default::default()
        };
        for name in [
            "require",
            "module",
            "exports",
            "__filename",
            "__dirname",
            "__import",
            "__importMeta",
        ] {
            let s = unit.locals;
            unit.locals += 1;
            unit.symbol_map.insert(name.to_owned(), s);
        }
        let exports_slot = unit.symbol_map["exports"];

        for stmt in &program.body {
            self.compile_esm_stmt(stmt, &mut unit, exports_slot);
        }
        // `__esModule` 标记（Node require(esm) 返回对象含该键）
        let key = crate::codegen::add_constant(
            &mut unit,
            aluka_bytecode::Constant::String("__esModule".to_owned()),
        );
        unit.code
            .push(Instr::new(Op::LoadLocal, exports_slot as u32));
        unit.code.push(Instr::new(Op::PushTrue, 0));
        unit.code.push(Instr::new(Op::SetProp, key));
        unit.code.push(Instr::new(Op::Pop, 0));
        // 此处不提前 RETURN_UNDEF：wrapper 以 exports 收口（见下方 Return）
        // 递归回填闭包表达式占位指令（与 Script/CJS 路径同款）
        while let Some((instr_idx, closure_def, mut parent_info)) = unit.closure_backpatches.pop() {
            for (k, v) in &unit.symbol_map {
                parent_info.locals.entry(k.clone()).or_insert(*v);
            }
            for (k, v) in &unit.upvalue_map {
                parent_info.upvalues.entry(k.clone()).or_insert(*v);
            }
            let child_idx = self.compile_function_with_parent(&closure_def, Some(&parent_info));
            unit.code[instr_idx].operand = child_idx as u32;
        }
        // wrapper 以 exports 对象收口：异步完成（TLA/await import）时
        // 完成值即 exports，加载器的 import promise 链以此兑现依赖命名空间
        unit.code
            .push(Instr::new(Op::LoadLocal, exports_slot as u32));
        unit.code.push(Instr::new(Op::Return, 0));
        let wrapper_idx = self.functions.len();
        let had_direct_eval = unit.has_direct_eval;
        let symbol_map = unit.symbol_map.clone();
        let upvalue_map = unit.upvalue_map.clone();
        let mut wrapper_tpl = unit.to_func_template("main");
        // ESM wrapper 常态 async：顶层 await（TLA）与 import 的 await 挂起
        // 依赖异步帧收割/恢复；无挂起时函数体同步执行完，require(esm) 的
        // 同步 exports 可见性不受影响
        wrapper_tpl.is_async = true;
        if had_direct_eval {
            attach_direct_eval_marker(&mut wrapper_tpl, &symbol_map);
            attach_direct_eval_upvalue_marker(&mut wrapper_tpl, &upvalue_map);
        }
        self.functions.push(wrapper_tpl);
        self.header_extras.push(FuncHeaderExtras {
            arguments_slot: -1,
            no_arguments_object: true,
            new_target_slot: -1,
            inlinable: false,
        });

        // main：MAKE_CLOSURE(wrapper) + RETURN
        self.functions[0].code = vec![
            Instr::new(Op::MakeClosure, wrapper_idx as u32),
            Instr::new(Op::Return, 0),
        ];
        self.functions[0].num_locals = 1;
        self.functions[0].max_stack = 8;

        if self.header_extras.iter().all(|e| {
            e.arguments_slot == -1
                && e.no_arguments_object
                && e.new_target_slot == -1
                && !e.inlinable
        }) {
            self.header_extras.clear();
        }

        BytecodeModule {
            header_extras: std::mem::take(&mut self.header_extras),
            version: 30,
            functions: std::mem::take(&mut self.functions),
            classes: std::mem::take(&mut self.classes),
        }
    }

    /// ESM 语句编译：`export` 绑定到 `exports` 槽，其余走普通语句编译。
    fn compile_esm_stmt(&mut self, stmt: &Stmt, unit: &mut CompiledUnit, exports_slot: usize) {
        use aluka_parser::ast::ExportDecl;
        match stmt {
            Stmt::Export(ExportDecl::Named {
                decl: Some(inner),
                specifiers,
                ..
            }) => {
                // 内嵌声明（var/let/const/function/class）先声明到局部，再挂 exports
                match inner.as_ref() {
                    Stmt::Function(func_def) => {
                        let parent_info =
                            ParentScopeInfo::new(unit.symbol_map.clone(), unit.upvalue_map.clone());
                        let fn_idx =
                            self.compile_function_with_parent(func_def, Some(&parent_info));
                        let slot = if let Some(&s) = unit.symbol_map.get(&func_def.name) {
                            s
                        } else {
                            let s = unit.locals;
                            unit.locals += 1;
                            unit.symbol_map.insert(func_def.name.clone(), s);
                            s
                        };
                        unit.code.push(Instr::new(Op::MakeClosure, fn_idx as u32));
                        unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                        self.emit_export_prop(unit, exports_slot, slot, &func_def.name);
                    }
                    other => {
                        compile_stmt(other, unit, false);
                        if let Stmt::VarDecl { name, .. } = other {
                            if let Some(&slot) = unit.symbol_map.get(name) {
                                self.emit_export_prop(unit, exports_slot, slot, name);
                            }
                        }
                    }
                }
                for spec in specifiers {
                    if let Some(&slot) = unit.symbol_map.get(&spec.local) {
                        self.emit_export_prop(unit, exports_slot, slot, &spec.exported);
                    }
                }
            }
            Stmt::Export(ExportDecl::Named {
                decl: None,
                specifiers,
                ..
            }) => {
                for spec in specifiers {
                    if let Some(&slot) = unit.symbol_map.get(&spec.local) {
                        self.emit_export_prop(unit, exports_slot, slot, &spec.exported);
                    }
                }
            }
            Stmt::Export(ExportDecl::Default(expr)) => {
                let key = crate::codegen::add_constant(
                    unit,
                    aluka_bytecode::Constant::String("default".to_owned()),
                );
                unit.code
                    .push(Instr::new(Op::LoadLocal, exports_slot as u32));
                compile_expr(expr, unit);
                unit.code.push(Instr::new(Op::SetProp, key));
                unit.code.push(Instr::new(Op::Pop, 0));
            }
            Stmt::Export(ExportDecl::All { .. }) => {
                // 命名空间重导出暂不支持：静默忽略（不会错误绑定）
            }
            Stmt::Import(decl) => {
                // M2.2 异步模块加载器 DAG：import 编译为
                //   var __ns_N = await __aluka_import__(source);
                //   var <local> = __ns_N.<imported>;   （快照绑定）
                // wrapper 常态 async（见 compile_esm）——依赖模块含 TLA 时
                // __aluka_import__ 返回其完成 Promise，await 挂起导入方帧，
                // 由 promise resume 链在依赖完成后继续（DAG 涌现于事件循环）
                let ns = format!("__aluka_ns_{}", self.esm_import_counter);
                self.esm_import_counter += 1;
                let ns_decl = Stmt::VarDecl {
                    name: ns.clone(),
                    init: Some(Expr::Await(Box::new(Expr::Call {
                        callee: Box::new(Expr::Ident("__aluka_import__".to_owned())),
                        args: vec![Expr::String(decl.source.clone())],
                    }))),
                    kind: VarKind::Var,
                };
                compile_stmt(&ns_decl, unit, false);
                for spec in &decl.specifiers {
                    let (local, imported) = match spec {
                        aluka_parser::ast::ImportSpecifier::Named { local, imported } => {
                            (local.clone(), imported.clone())
                        }
                        aluka_parser::ast::ImportSpecifier::Default(name) => {
                            (name.clone(), "default".to_owned())
                        }
                        aluka_parser::ast::ImportSpecifier::Namespace(name) => {
                            // 整包导入：ns 即命名空间对象
                            let bind = Stmt::VarDecl {
                                name: name.clone(),
                                init: Some(Expr::Ident(ns.clone())),
                                kind: VarKind::Var,
                            };
                            compile_stmt(&bind, unit, false);
                            continue;
                        }
                    };
                    let bind = Stmt::VarDecl {
                        name: local,
                        init: Some(Expr::Member {
                            obj: Box::new(Expr::Ident(ns.clone())),
                            prop: imported,
                        }),
                        kind: VarKind::Var,
                    };
                    compile_stmt(&bind, unit, false);
                }
            }
            other => {
                compile_stmt(other, unit, false);
            }
        }
    }

    /// 发射 `exports.<name> = <slot 值>`（栈序：obj 先压，val 后压，SetProp 弹出）。
    fn emit_export_prop(
        &mut self,
        unit: &mut CompiledUnit,
        exports_slot: usize,
        value_slot: usize,
        name: &str,
    ) {
        let key =
            crate::codegen::add_constant(unit, aluka_bytecode::Constant::String(name.to_owned()));
        unit.code
            .push(Instr::new(Op::LoadLocal, exports_slot as u32));
        unit.code.push(Instr::new(Op::LoadLocal, value_slot as u32));
        unit.code.push(Instr::new(Op::SetProp, key));
        unit.code.push(Instr::new(Op::Pop, 0));
    }

    /// 编译单一函数定义（无父级上下文）
    pub fn compile_function(&mut self, def: &FunctionDef) -> usize {
        self.compile_function_with_parent(def, None)
    }

    /// 编译函数定义，并支持向父级作用域捕获闭包变量（Upvalues）
    pub fn compile_function_with_parent(
        &mut self,
        def: &FunctionDef,
        parent_scope: Option<&ParentScopeInfo>,
    ) -> usize {
        self.compile_method_function(def, parent_scope, None)
    }

    /// 编译方法函数定义，支持向父级作用域捕获闭包变量及派生类的父类上值
    pub fn compile_method_function(
        &mut self,
        def: &FunctionDef,
        parent_scope: Option<&ParentScopeInfo>,
        class_id: Option<usize>,
    ) -> usize {
        let num_params = if def.is_var_args && !def.params.is_empty() {
            (def.params.len() - 1) as u32
        } else {
            def.params.len() as u32
        };
        let mut unit = CompiledUnit {
            locals: 1, // locals[0] 保留给 this
            num_params,
            is_var_args: def.is_var_args,
            class_id,
            ..Default::default()
        };
        for param in &def.params {
            let s = unit.locals;
            unit.locals += 1;
            unit.symbol_map.insert(param.clone(), s);
        }

        // 若类拥有父类，将外层声明的 __home_ctor_{cid}__ 和 __home_proto_{cid}__ 预置为闭包 Upvalue
        if let (Some(cid), Some(parent_info)) = (class_id, parent_scope) {
            let ctor_sym = format!("__home_ctor_{cid}__");
            if let Some(&parent_slot) = parent_info.locals.get(&ctor_sym) {
                let uv_idx = unit.upvalues.len();
                unit.upvalues.push(UpvalueCapture {
                    is_local: true,
                    index: parent_slot as u32,
                });
                unit.upvalue_map.insert(ctor_sym, uv_idx);
            }
            let proto_sym = format!("__home_proto_{cid}__");
            if let Some(&parent_slot) = parent_info.locals.get(&proto_sym) {
                let uv_idx = unit.upvalues.len();
                unit.upvalues.push(UpvalueCapture {
                    is_local: true,
                    index: parent_slot as u32,
                });
                unit.upvalue_map.insert(proto_sym, uv_idx);
            }
        }

        // `arguments` 对象：函数体（含嵌套箭头函数）引用且未被自身声明遮蔽时，
        // 分配一个局部槽位，运行时由 VM 按 header_extras.arguments_slot 注入
        // 实参数组（快照语义，对齐 Go 前端）。
        // 箭头函数不绑定自己的 arguments——其引用经上值捕获落到外层函数槽。
        let args_slot = {
            let mut uses = Vec::new();
            for stmt in &def.body {
                collect_ident_uses(stmt, &mut uses);
            }
            let references = uses.iter().any(|n| n == "arguments");
            let shadowed = def.params.iter().any(|p| p == "arguments")
                || def.body.iter().any(|s| stmt_declares(s, "arguments"));
            if references && !shadowed && !def.is_arrow {
                let s = unit.locals;
                unit.locals += 1;
                unit.symbol_map.insert("arguments".to_owned(), s);
                Some(s as i32)
            } else {
                None
            }
        };

        // 预注册函数体内顶层绑定槽位（var/let/const/解构/class/嵌套函数）
        for stmt in def.body.iter() {
            match stmt {
                Stmt::Function(func_def) => {
                    ensure_slot(&mut unit, &func_def.name);
                }
                Stmt::VarDecl { name, .. } => {
                    ensure_slot(&mut unit, name);
                }
                Stmt::MultiVarDecl { decls, .. } => {
                    for (name, _) in decls {
                        ensure_slot(&mut unit, name);
                    }
                }
                Stmt::DestructureDecl { pattern, .. } => {
                    ensure_pattern_slots(&mut unit, pattern);
                }
                Stmt::Class { name, .. } => {
                    ensure_slot(&mut unit, name);
                }
                _ => {}
            }
        }
        // 块内函数声明（递归收集）同样提升到本函数作用域：先预注册槽位，使下面的
        // 上值捕获识别（`parent_scope` 快照）与提升编译都能看到这些名字。
        {
            let mut block_fns: Vec<&Stmt> = Vec::new();
            collect_scope_functions(&def.body, &mut block_fns);
            for stmt in &block_fns {
                if let Stmt::Function(func_def) = stmt {
                    ensure_slot(&mut unit, &func_def.name);
                }
            }
        }

        // 若存在父级符号表，预先识别并建立闭包上值捕获（Upvalues，包括直接局部变量与跨层上值继承）
        if let Some(parent_info) = parent_scope {
            let mut uses = Vec::new();
            for stmt in &def.body {
                collect_ident_uses(stmt, &mut uses);
            }
            for name in uses {
                if !unit.symbol_map.contains_key(&name) {
                    if let Some(&parent_slot) = parent_info.locals.get(&name) {
                        if !unit.upvalue_map.contains_key(&name) {
                            let uv_idx = unit.upvalues.len();
                            unit.upvalues.push(UpvalueCapture {
                                is_local: true,
                                index: parent_slot as u32,
                            });
                            unit.upvalue_map.insert(name, uv_idx);
                        }
                    } else if let Some(&parent_uv) = parent_info.upvalues.get(&name) {
                        if !unit.upvalue_map.contains_key(&name) {
                            let uv_idx = unit.upvalues.len();
                            unit.upvalues.push(UpvalueCapture {
                                is_local: false,
                                index: parent_uv as u32,
                            });
                            unit.upvalue_map.insert(name, uv_idx);
                        }
                    }
                }
            }
        }

        // 函数体：嵌套 function 声明提升（先绑定闭包，再编译其余语句）。
        // 收集为**递归**（含 `if`/`for`/普通块内的声明）——块内函数同样提升到本
        // 函数作用域（Node 实测：块内可调用、块外可访问），故必须进入块语句。
        let mut hoisted_fns: Vec<&Stmt> = Vec::new();
        let mut ordered_stmts: Vec<&Stmt> = Vec::new();
        collect_scope_functions(&def.body, &mut hoisted_fns);
        for stmt in def.body.iter() {
            if !matches!(stmt, Stmt::Function(_)) {
                ordered_stmts.push(stmt);
            }
        }
        for stmt in &hoisted_fns {
            if let Stmt::Function(child_def) = stmt {
                let parent_info =
                    ParentScopeInfo::new(unit.symbol_map.clone(), unit.upvalue_map.clone());
                let child_idx = self.compile_function_with_parent(child_def, Some(&parent_info));
                let slot = if let Some(&s) = unit.symbol_map.get(&child_def.name) {
                    s
                } else {
                    let s = unit.locals;
                    unit.locals += 1;
                    unit.symbol_map.insert(child_def.name.clone(), s);
                    s
                };
                unit.code
                    .push(Instr::new(Op::MakeClosure, child_idx as u32));
                unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
            }
        }

        for (i, stmt) in ordered_stmts.iter().enumerate() {
            let is_last = i == ordered_stmts.len() - 1;
            match stmt {
                Stmt::Function(child_def) => {
                    let parent_info =
                        ParentScopeInfo::new(unit.symbol_map.clone(), unit.upvalue_map.clone());
                    let child_idx =
                        self.compile_function_with_parent(child_def, Some(&parent_info));
                    let slot = if let Some(&s) = unit.symbol_map.get(&child_def.name) {
                        s
                    } else {
                        let s = unit.locals;
                        unit.locals += 1;
                        unit.symbol_map.insert(child_def.name.clone(), s);
                        s
                    };
                    unit.code
                        .push(Instr::new(Op::MakeClosure, child_idx as u32));
                    unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                }
                // 类声明（**函数作用域内**）：镜像顶层装配。
                //
                // 此前 `Stmt::Class` 落到下方 `compile_stmt` 的空实现
                // （codegen.rs 的 `Stmt::Function | Stmt::Class => { ... }` 只处理
                // 栈平衡，注释称「在 compile_module 中提取」），而提取只发生在
                // **模块顶层**语句上——于是函数体内 `class A { m() {} }` 既不绑定
                // 名字也不挂方法：`typeof A` → undefined、`new A().m` → undefined。
                // `new A()` 之所以「看似可用」，是因为 `new <未声明标识符>` 有宽松
                // 回退（`new TotallyUndeclared()` 也不报错），并非绑定真的存在。
                Stmt::Class {
                    name,
                    super_class,
                    constructor,
                    methods,
                } => {
                    let class_id = self.classes.len();
                    if let Some(super_expr) = super_class {
                        compile_expr(super_expr, &mut unit);
                        unit.code.push(Instr::new(Op::Dup, 0));

                        let ctor_sym = format!("__home_ctor_{class_id}__");
                        let ctor_slot = unit.locals;
                        unit.locals += 1;
                        unit.symbol_map.insert(ctor_sym, ctor_slot);
                        unit.code.push(Instr::new(Op::StoreLocal, ctor_slot as u32));

                        unit.code.push(Instr::new(Op::LoadLocal, ctor_slot as u32));
                        let proto_idx = crate::codegen::add_constant(
                            &mut unit,
                            aluka_bytecode::Constant::String("prototype".to_owned()),
                        );
                        unit.code.push(Instr::new(Op::GetProp, proto_idx));
                        let proto_sym = format!("__home_proto_{class_id}__");
                        let proto_slot = unit.locals;
                        unit.locals += 1;
                        unit.symbol_map.insert(proto_sym, proto_slot);
                        unit.code
                            .push(Instr::new(Op::StoreLocal, proto_slot as u32));
                    }

                    let parent_info =
                        ParentScopeInfo::new(unit.symbol_map.clone(), unit.upvalue_map.clone());
                    let class_idx = self.compile_class(
                        name,
                        super_class.is_some(),
                        constructor,
                        methods,
                        Some(&parent_info),
                        class_id,
                    );
                    unit.code.push(Instr::new(Op::MakeClass, class_idx as u32));
                    let slot = if let Some(&s) = unit.symbol_map.get(name) {
                        s
                    } else {
                        let s = unit.locals;
                        unit.locals += 1;
                        unit.symbol_map.insert(name.clone(), s);
                        s
                    };
                    unit.code.push(Instr::new(Op::StoreLocal, slot as u32));
                }
                other => {
                    compile_stmt(other, &mut unit, is_last);
                }
            }
        }

        // 递归回填该函数单元中所有的闭包表达式占位指令
        while let Some((instr_idx, closure_def, mut parent_info)) = unit.closure_backpatches.pop() {
            for (k, v) in &unit.symbol_map {
                parent_info.locals.entry(k.clone()).or_insert(*v);
            }
            for (k, v) in &unit.upvalue_map {
                parent_info.upvalues.entry(k.clone()).or_insert(*v);
            }
            let child_idx = self.compile_function_with_parent(&closure_def, Some(&parent_info));
            unit.code[instr_idx].operand = child_idx as u32;
        }

        if unit.code.is_empty()
            || !matches!(
                unit.code.last().map(|i| i.op),
                Some(Op::Return | Op::ReturnUndef)
            )
        {
            unit.code.push(Instr::new(Op::ReturnUndef, 0));
        }
        // 直接求值词法槽位降级：函数体含裸 `eval(...)` 时，强制把父级
        // 全部局部捕获为上值（未被代码引用的绑定也保留地址），使求值体
        // 可经上值外溢访问外层词法绑定（对齐引擎的 scope degradation）
        if unit.has_direct_eval {
            if let Some(parent) = parent_scope {
                for (name, &slot) in &parent.locals {
                    if unit.symbol_map.contains_key(name) || unit.upvalue_map.contains_key(name) {
                        continue;
                    }
                    let uv_idx = unit.upvalues.len();
                    unit.upvalues.push(UpvalueCapture {
                        is_local: true,
                        index: slot as u32,
                    });
                    unit.upvalue_map.insert(name.clone(), uv_idx);
                }
                // 父级已存在的上值链继续透传（祖父级绑定可达）
                for (name, &uv) in &parent.upvalues {
                    if unit.upvalue_map.contains_key(name) {
                        continue;
                    }
                    let idx = unit.upvalues.len();
                    unit.upvalues.push(UpvalueCapture {
                        is_local: false,
                        index: uv as u32,
                    });
                    unit.upvalue_map.insert(name.clone(), idx);
                }
            }
        }
        let had_direct_eval = unit.has_direct_eval;
        let symbol_map = unit.symbol_map.clone();
        let upvalue_map = unit.upvalue_map.clone();
        let mut func_tpl = unit.to_func_template(&def.name);
        func_tpl.is_async = def.is_async;
        func_tpl.is_generator = def.is_generator;
        if had_direct_eval {
            attach_direct_eval_marker(&mut func_tpl, &symbol_map);
            attach_direct_eval_upvalue_marker(&mut func_tpl, &upvalue_map);
        }
        let idx = self.functions.len();
        self.functions.push(func_tpl);
        self.header_extras.push(FuncHeaderExtras {
            arguments_slot: args_slot.unwrap_or(-1),
            no_arguments_object: args_slot.is_none(),
            new_target_slot: -1,
            inlinable: false,
        });
        idx
    }

    /// 编译类定义，返回类在 classes 中的索引
    pub fn compile_class(
        &mut self,
        name: &str,
        has_super: bool,
        constructor: &Option<FunctionDef>,
        methods: &[ClassMethodDef],
        parent_scope: Option<&ParentScopeInfo>,
        class_id: usize,
    ) -> usize {
        let ctor_idx = if let Some(ctor_def) = constructor {
            self.compile_method_function(ctor_def, parent_scope, Some(class_id)) as u32
        } else if has_super {
            let def = FunctionDef {
                name: format!("{name}_constructor"),
                params: vec!["__args__".to_owned()],
                is_var_args: true,
                body: vec![Stmt::Expr(Expr::Call {
                    callee: Box::new(Expr::Super),
                    args: vec![Expr::Spread(Box::new(Expr::Ident("__args__".to_owned())))],
                })],
                is_async: false,
                is_generator: false,
                is_arrow: false,
            };
            self.compile_method_function(&def, parent_scope, Some(class_id)) as u32
        } else {
            let def =
                FunctionDef::new(format!("{name}_constructor"), Vec::new(), false, Vec::new());
            self.compile_method_function(&def, parent_scope, Some(class_id)) as u32
        };

        let mut class_methods = Vec::with_capacity(methods.len());
        for m in methods {
            let fn_def = FunctionDef::new(
                format!("{name}_{}", m.name),
                m.params.clone(),
                false,
                m.body.clone(),
            );
            let func_index =
                self.compile_method_function(&fn_def, parent_scope, Some(class_id)) as u32;
            class_methods.push(ClassMethod {
                name: m.name.clone(),
                func_index,
                is_static: m.is_static,
                kind: m.kind,
            });
        }

        let class_tpl = ClassTemplate {
            name: name.to_owned(),
            has_super,
            constructor_index: ctor_idx,
            methods: class_methods,
            computed_indices: Vec::new(),
        };

        let idx = self.classes.len();
        self.classes.push(class_tpl);
        idx
    }
}

/// 递归收集「当前函数作用域内」的函数声明（**含嵌套块内的声明**），先序。
///
/// 语义依据（Node.js 22 LTS 实测）：块内 `function` 声明在 sloppy 模式下被提升到
/// 最近的**函数作用域**——块内可调用，块外亦可访问：
///
/// ```js
/// if (true) { function f() {} }
/// typeof f   // Node: "function"（非 undefined）
/// ```
///
/// 而提升收集此前只遍历**直接子语句**（`if`/`for`/普通块内的声明被漏掉），且
/// `compile_stmt` 对 `Stmt::Function` 是空实现（只保证栈平衡），于是块内函数
/// 「既不绑定名字也不可用」——`typeof f` 为 `undefined`（引擎级缺陷，见
/// `.work/TODO/20260911/README.md` §12.2）。
///
/// 递归边界：
/// * **进入**块语句（`Block`）与可含语句块的语句（`if`/`while`/`do-while`/`for`/
///   `for-in`/`for-of`/`try`/`switch`/`export`）；
/// * **不进入**嵌套函数的函数体——那属于子函数自己的提升域（子函数编译时会各自调用
///   本函数收集）；
/// * 表达式内部的函数是**函数表达式**而非声明，故不进入表达式。
fn collect_scope_functions<'a>(stmts: &'a [Stmt], out: &mut Vec<&'a Stmt>) {
    for stmt in stmts {
        collect_scope_functions_in_stmt(stmt, out);
    }
}

/// [`collect_scope_functions`] 的单语句递归分支。
fn collect_scope_functions_in_stmt<'a>(stmt: &'a Stmt, out: &mut Vec<&'a Stmt>) {
    match stmt {
        Stmt::Function(_) => out.push(stmt),
        Stmt::Block(body) => collect_scope_functions(body, out),
        Stmt::If {
            then_branch,
            else_branch,
            ..
        } => {
            collect_scope_functions_in_stmt(then_branch, out);
            if let Some(else_stmt) = else_branch {
                collect_scope_functions_in_stmt(else_stmt, out);
            }
        }
        Stmt::While { body, .. }
        | Stmt::DoWhile { body, .. }
        | Stmt::ForIn { body, .. }
        | Stmt::ForOf { body, .. } => collect_scope_functions_in_stmt(body, out),
        Stmt::For { init, body, .. } => {
            if let Some(init_stmt) = init {
                collect_scope_functions_in_stmt(init_stmt, out);
            }
            collect_scope_functions_in_stmt(body, out);
        }
        Stmt::Try {
            body,
            catch_body,
            finally_body,
            ..
        } => {
            collect_scope_functions_in_stmt(body, out);
            if let Some(catch_stmt) = catch_body {
                collect_scope_functions_in_stmt(catch_stmt, out);
            }
            if let Some(finally_stmt) = finally_body {
                collect_scope_functions_in_stmt(finally_stmt, out);
            }
        }
        Stmt::Switch { cases, .. } => {
            for case in cases {
                collect_scope_functions(&case.consequent, out);
            }
        }
        Stmt::Export(aluka_parser::ast::ExportDecl::Named {
            decl: Some(inner), ..
        }) => collect_scope_functions_in_stmt(inner, out),
        _ => {}
    }
}
/// 预注册符号槽位（已存在则复用）。
fn ensure_slot(unit: &mut CompiledUnit, name: &str) {
    if name.is_empty() {
        return;
    }
    if !unit.symbol_map.contains_key(name) {
        let s = unit.locals;
        unit.locals += 1;
        unit.symbol_map.insert(name.to_owned(), s);
    }
}

/// 递归注册解构模式的全部标识符槽位。
fn ensure_pattern_slots(unit: &mut CompiledUnit, pattern: &VarPattern) {
    match pattern {
        VarPattern::Ident(name) => ensure_slot(unit, name),
        VarPattern::Array(elems) => {
            for elem in elems {
                ensure_slot(unit, &elem.name);
            }
        }
        VarPattern::Object(props) => {
            for prop in props {
                ensure_pattern_slots(unit, &prop.value);
            }
        }
    }
}

/// 直接求值降级标记：把局部名表按槽位序编码进模板常量池。
///
/// 仅当函数体含裸标识符 `eval(...)` 调用形态时调用（`has_direct_eval`）。
/// 编码格式：`__aluka_locals__\u{1}` 前缀 + `\u{1}` 分隔的名字序列，
/// 序列第 i 项对应局部槽位 `i + 1`（slot 0 = this 不入表；空洞为空串）。
/// 运行时直接求值据此物化当前帧词法作用域，实现穿透与写回。
/// 直接求值降级标记（上值面）：把上值名表按 Upvalue 索引序编码进模板
/// 常量池（`__aluka_upvals__` 前缀 + SEP 分隔）。嵌套函数内的直接求值
/// 经此外溢捕获外层词法绑定（读与写回均经上值句柄共享）。
fn attach_direct_eval_upvalue_marker(
    func_tpl: &mut FuncTemplate,
    upvalue_map: &std::collections::HashMap<String, usize>,
) {
    const SEP: char = '\u{1}';
    let max_idx = upvalue_map.values().copied().max().unwrap_or(0);
    let mut names = vec![String::new(); max_idx + 1];
    for (name, &idx) in upvalue_map {
        if idx < names.len() {
            names[idx] = name.clone();
        }
    }
    let marker = format!(
        "__aluka_upvals__{}{}",
        SEP,
        names.join(SEP.to_string().as_str())
    );
    func_tpl.constants.push(Constant::String(marker));
}

fn attach_direct_eval_marker(
    func_tpl: &mut FuncTemplate,
    symbol_map: &std::collections::HashMap<String, usize>,
) {
    let max_slot = symbol_map.values().copied().max().unwrap_or(0);
    let mut names = vec![String::new(); max_slot];
    for (name, &slot) in symbol_map {
        if slot >= 1 {
            let i = slot - 1;
            if i < names.len() {
                names[i] = name.clone();
            }
        }
    }
    let marker = format!("__aluka_locals__\u{1}{}", names.join("\u{1}"));
    func_tpl.constants.push(Constant::String(marker));
}
