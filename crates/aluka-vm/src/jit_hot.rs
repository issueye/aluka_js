//! Tier 1 热点触发：per-函数调用计数 → 达阈值编译 → 命中即执行机器码。
//!
//! 策略（ADR 0005 决定 4 的 J2 收口）：
//! - **函数级计数**，无 OSR：只在函数**入口**切换执行体，正在执行的帧不迁移；
//! - **一次性编译**：失败（子集外操作码等）即标记 [`JitSlot::Rejected`]，
//!   永不重试——避免热路径反复吃编译开销；
//! - **资格判定从严**：生成器/async/varargs/try/arguments 对象一律不编译，
//!   这些语义在 JIT 子集外（帧挂起、异常展开、rest 数组）；
//! - **缓存随模块重置**：`run_module` 替换 `module_functions` 时清空，
//!   否则函数索引错位会执行到别的函数的机器码。
//!
//! 本模块含 AGENTS 允许的 JIT 边界 unsafe（复用装箱 ctx 的裸指针传递），
//! 作用域收敛在 [`Vm::jit_run`] 内。

#![allow(unsafe_code)]

use crate::interpreter::Vm;
use crate::value::{Value, ValueCase};
use aluka_bytecode::FuncTemplate;
use aluka_jit::JittedFn;
use std::rc::Rc;

/// 触发 JIT 编译的调用次数阈值。
///
/// 取值权衡：太低则冷函数白吃编译开销，太高则热函数迟迟跑不到机器码。
/// 50 次可让基准与真实热循环在首个数量级内完成升级。
pub const JIT_HOT_THRESHOLD: u32 = 50;

/// JIT 代数分配器（**进程级**单调递增）。
///
/// 调用内联缓存（[`aluka_jit::ctx::CallCell`]）登记的是**裸入口地址**，其存活
/// 由持有 `Rc<JittedFn>` 的 `jit_slots` 保证。代数守卫必须能识别「缓存所属的
/// 那一份函数表已经消失」——包括**换了另一个 `Vm`** 的情况：同一份编译产物
/// 可被多个 `Vm` 驱动（测试与基准就这么用），而两个 `Vm` 里同下标闭包的对象
/// 句柄很可能相同，仅靠 per-Vm 计数会让上一个 `Vm` 的 stale 入口通过守卫。
/// 因此代数取自进程级计数器，`Vm::new` 与每次 `jit_reset` 各取一个新值。
static JIT_GEN_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// 取下一个 JIT 代数。
pub(crate) fn next_jit_gen() -> u32 {
    JIT_GEN_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// 单个函数的 JIT 状态。
#[derive(Default)]
pub enum JitSlot {
    /// 未达阈值（携带调用计数）
    #[default]
    Cold,
    /// 已编译，可执行机器码
    Compiled(Rc<JittedFn>),
    /// 编译被拒（子集外操作码 / 资格不符），不再重试
    Rejected,
}

impl std::fmt::Debug for JitSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cold => write!(f, "Cold"),
            Self::Compiled(_) => write!(f, "Compiled"),
            Self::Rejected => write!(f, "Rejected"),
        }
    }
}

/// 函数模板是否具备 JIT 资格（语义在子集内）。
///
/// 只做**静态形状**判定；指令级子集判定由 `aluka_jit::jit_compile` 负责
/// （遇子集外操作码返回 `UnsupportedOpcode`，调用方转 `Rejected`）。
#[must_use]
pub fn is_jit_eligible(tmpl: &FuncTemplate, has_arguments_object: bool) -> bool {
    !tmpl.is_generator
        && !tmpl.is_async
        && !tmpl.is_var_args
        && tmpl.try_table.is_empty()
        && !has_arguments_object
        && tmpl.num_locals >= 1
}

impl Vm {
    /// 计数并按需编译；返回可执行机器码（`None` = 走解释器）。
    ///
    /// 调用点在 `invoke_function` 的帧建立之后——此时 `self.locals` 已绑定
    /// 实参，但 JIT 走自己的 `(ctx, args, len)` ABI，不读解释器帧。
    pub(crate) fn jit_lookup(&mut self, func_idx: usize) -> Option<Rc<JittedFn>> {
        if self.jit_slots.len() < self.module_functions.len() {
            self.jit_slots
                .resize_with(self.module_functions.len(), JitSlot::default);
            self.jit_counters.resize(self.module_functions.len(), 0);
        }
        match self.jit_slots.get(func_idx) {
            Some(JitSlot::Compiled(f)) => return Some(f.clone()),
            Some(JitSlot::Rejected) => return None,
            Some(JitSlot::Cold) => {}
            None => return None,
        }
        // 计数未到阈值：继续解释执行
        let count = {
            let c = self.jit_counters.get_mut(func_idx)?;
            *c = c.saturating_add(1);
            *c
        };
        if count < JIT_HOT_THRESHOLD {
            return None;
        }
        // 达阈值：资格判定 + 一次性编译
        let tmpl = self.module_functions.get(func_idx)?.clone();
        let has_args_obj = self
            .module_header_extras
            .get(func_idx)
            .is_some_and(|e| e.arguments_slot >= 0 && !e.no_arguments_object);
        if !is_jit_eligible(&tmpl, has_args_obj) {
            self.jit_slots[func_idx] = JitSlot::Rejected;
            return None;
        }
        let consts = self.module_constants[func_idx].clone();
        let ctx = self.build_jit_ctx(&consts);
        match aluka_jit::jit_compile(&tmpl, &ctx.vtable) {
            Ok(f) => {
                let rc = Rc::new(f);
                self.jit_slots[func_idx] = JitSlot::Compiled(rc.clone());
                Some(rc)
            }
            Err(_) => {
                self.jit_slots[func_idx] = JitSlot::Rejected;
                None
            }
        }
    }

    /// 以机器码执行一次调用：实参装箱 → 调用 → 结果拆箱。
    pub(crate) fn jit_run(
        &mut self,
        func_idx: usize,
        jit: &Rc<JittedFn>,
        args: &[Value],
        num_params: usize,
        upvalues: Vec<crate::value::Upvalue>,
    ) -> Value {
        // 实参装箱：≤8 参走栈数组（避免每次调用的堆分配——closureCall 这类
        // 每轮都进 JIT 的负载上，这项分配是主开销之一）。
        // 长度取**形参数**而非实参数：JIT 侧按形参数逐槽读 `args_ptr`，少传
        // 时必须以 undefined 补齐，否则读到数组之外。
        let mut inline: [u64; 8] = [aluka_jit::valbox::UNDEFINED; 8];
        let given = num_params.min(args.len());
        let heap_boxes: Vec<u64>;
        let boxes: &[u64] = if num_params <= 8 {
            for (i, v) in args.iter().take(given).enumerate() {
                inline[i] = crate::jit_helpers::from_vm_value(*v);
            }
            &inline[..num_params]
        } else {
            heap_boxes = (0..num_params)
                .map(|i| {
                    args.get(i).map_or(aluka_jit::valbox::UNDEFINED, |v| {
                        crate::jit_helpers::from_vm_value(*v)
                    })
                })
                .collect();
            &heap_boxes
        };
        // LOAD_UPVALUE helper 读 `current_upvalues`；`CALL` helper 可能重入
        // 解释器（并再次换帧），故必须像解释器一样保存/恢复本帧上值与常量池。
        // 常量池以 Rc 换入换出（引用计数增减，无深拷贝）。
        let consts = Rc::clone(&self.module_constants[func_idx]);
        let old_constants = std::mem::replace(&mut self.current_constants, Rc::clone(&consts));
        // 换出的外层上值登记保存帧寄存器（重入解释器触发 GC 时保持存活）
        self.gc_saved_frames.push(crate::gc::SavedFrameState {
            upvalues: std::mem::replace(&mut self.current_upvalues, upvalues),
            ..Default::default()
        });

        // 复用装箱 ctx：首次构造完整结构，之后只改随帧变化的三字段，并在
        // 调用前后保存/恢复（嵌套 JIT→JIT 调用会共享同一 ctx，不恢复会让
        // 外层帧读到内层的常量池）。
        if self.jit_ctx.is_none() {
            let fresh = self.build_jit_ctx(&consts);
            self.jit_ctx = Some(Box::new(fresh));
        }
        let vm_ptr = self as *mut Vm as *mut core::ffi::c_void;
        let heap_ptr = self.heap.as_ptr() as *const u8;
        let cur_gen = self.jit_gen;
        let globals_gen = self.globals_epoch32();
        let frames_ptr = &mut self.jit_frames as *mut u32;
        let ctx_box = self.jit_ctx.as_mut().expect("上一步已初始化");
        let saved = (
            ctx_box.vm,
            ctx_box.consts_ptr,
            ctx_box.consts_len,
            ctx_box.heap_ptr,
        );
        ctx_box.vm = vm_ptr;
        ctx_box.consts_ptr = consts.as_ptr();
        ctx_box.consts_len = consts.len();
        ctx_box.heap_ptr = heap_ptr;
        // 代数不需保存恢复（属 Vm 状态而非帧状态），但复用的 ctx 可能建于上一代
        ctx_box.jit_gen = cur_gen;
        ctx_box.globals_gen = globals_gen;
        // 帧计数器地址：`Vm` 可能整体移动过（`build_jit_ctx` 时的地址已失效）
        ctx_box.frames_ptr = frames_ptr;
        let ctx_raw: *mut aluka_jit::ctx::JitCtx = &mut **ctx_box;
        // SAFETY: ctx_raw 指向 Vm 自有的装箱 ctx（地址稳定），JIT 与 helper
        // 在本次调用期间同步使用；helper 经 ctx.vm 回到同一 Vm（单线程契约）。
        let r = jit.call_ctx(unsafe { &mut *ctx_raw }, boxes);
        if let Some(ctx_box) = self.jit_ctx.as_mut() {
            ctx_box.vm = saved.0;
            ctx_box.consts_ptr = saved.1;
            ctx_box.consts_len = saved.2;
            ctx_box.heap_ptr = saved.3;
        }

        self.current_upvalues = self.gc_saved_frames.pop().unwrap_or_default().upvalues;
        self.current_constants = old_constants;
        crate::jit_helpers::to_vm_value(r)
    }

    /// JIT→JIT 直连调用：被调是**已编译且无上值**的闭包时直接执行机器码。
    ///
    /// 返回 `None` 表示不适用（非闭包/未编译/带上值/参数越界），调用方回退
    /// `invoke_callable` 的通用路径。跳过的是解释器帧建立开销——JIT 侧本就
    /// 不用解释器帧，`current_upvalues` 为空也无需保存恢复。
    pub(crate) fn jit_direct_call(&mut self, callee: Value, args: &[Value]) -> Option<Value> {
        let ValueCase::Object(r) = callee.case() else {
            return None;
        };
        let func_idx = match self.heap.get(r.0 as usize) {
            Some(crate::heap::HeapObject::Closure {
                func_idx, upvalues, ..
            }) if upvalues.is_empty() => *func_idx,
            _ => return None,
        };
        let jit = match self.jit_slots.get(func_idx) {
            Some(JitSlot::Compiled(f)) => f.clone(),
            // 未编译：交给通用路径（顺便让它累计热度）
            _ => return None,
        };
        let num_params = self.module_functions.get(func_idx)?.num_params as usize;
        Some(self.jit_run(func_idx, &jit, args, num_params, Vec::new()))
    }

    /// 活跃 JIT 帧数（`0` = 不在 JIT 帧内，GC 可正常回收）。
    ///
    /// 机器码在入口自增、返回点自减；回归用例据此断言「帧计数平衡」——
    /// 泄漏（永不归零）会让 GC 永久停摆，比误回收更难发现。
    #[must_use]
    pub fn jit_frames(&self) -> u32 {
        self.jit_frames
    }

    /// JIT `CALL` 落到 helper 的累计次数（调用 IC 未命中）。
    ///
    /// 原生直调命中时不进 helper，故本计数直接反映直调覆盖率：
    /// `N` 轮循环若只有首轮回退，计数为 1。
    #[must_use]
    pub fn jit_call_fallbacks(&self) -> u64 {
        self.jit_call_fallbacks
    }

    /// 已编译函数的机器码入口地址（调用 IC 登记用；未编译返回 `None`）。
    ///
    /// 入口在对应 `Rc<JittedFn>` 存活期间有效；`jit_reset` 丢弃编译产物时
    /// 递增 [`Vm::jit_gen`]，令一切登记过旧入口的缓存守卫失配。
    pub(crate) fn jit_entry_for(&self, func_idx: usize) -> Option<usize> {
        match self.jit_slots.get(func_idx) {
            Some(JitSlot::Compiled(f)) if !f.uses_upvalues => Some(f.entry_addr()),
            _ => None,
        }
    }

    /// 清空 JIT 缓存（模块替换时必须调用：函数索引语义改变）。
    ///
    /// 同时递增代数：已发射机器码里的调用 IC 记着上一代的入口地址，编译产物
    /// 随 `jit_slots` 释放后那些地址即失效，代数守卫是唯一拦截手段。
    pub(crate) fn jit_reset(&mut self) {
        self.jit_slots.clear();
        self.jit_counters.clear();
        self.jit_gen = next_jit_gen();
        if let Some(ctx) = self.jit_ctx.as_mut() {
            ctx.jit_gen = self.jit_gen;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aluka_bytecode::{BytecodeModule, Constant, Instr, Op};

    /// 构造纯数值函数模板：`fn(n) { return n * 2 + 1 }`。
    fn double_plus_one() -> FuncTemplate {
        FuncTemplate {
            name: "dbl".to_owned(),
            num_params: 1,
            num_locals: 2,
            is_var_args: false,
            is_generator: false,
            is_async: false,
            is_arrow: false,
            code: vec![
                Instr::new(Op::LoadLocal, 1),
                Instr::new(Op::PushConst, 0),
                Instr::new(Op::Mul, 0),
                Instr::new(Op::PushConst, 1),
                Instr::new(Op::Add, 0),
                Instr::new(Op::Return, 0),
            ],
            max_stack: 8,
            source_file: String::new(),
            constants: vec![Constant::Number(2.0), Constant::Number(1.0)],
            upvalues: Vec::new(),
            try_table: Vec::new(),
            line_table: Vec::new(),
        }
    }

    /// 顶层函数：反复调用 dbl 并累加（驱动热点计数跨阈值）。
    fn caller(rounds: u32) -> FuncTemplate {
        FuncTemplate {
            name: "main".to_owned(),
            num_params: 0,
            num_locals: 1,
            is_var_args: false,
            is_generator: false,
            is_async: false,
            is_arrow: false,
            code: vec![Instr::new(Op::PushInt, rounds), Instr::new(Op::Return, 0)],
            max_stack: 4,
            source_file: String::new(),
            constants: Vec::new(),
            upvalues: Vec::new(),
            try_table: Vec::new(),
            line_table: Vec::new(),
        }
    }

    /// 热点挂接：同一函数在阈值前后结果一致，且阈值后状态转为 `Compiled`。
    #[test]
    fn hot_function_switches_to_machine_code_with_identical_results() {
        let module = BytecodeModule {
            header_extras: Vec::new(),
            version: 30,
            functions: vec![caller(0), double_plus_one()],
            classes: Vec::new(),
        };
        let mut vm = Vm::new(0);
        vm.load_module_for_test(&module);
        let dbl_idx = 1usize;

        // 阈值前：解释执行（第 THRESHOLD 次调用即触发编译，故只跑 -1 次）
        let mut results = Vec::new();
        for i in 0..(JIT_HOT_THRESHOLD - 1) {
            let v = vm
                .invoke_function(
                    dbl_idx,
                    Value::Undefined,
                    &[Value::Number(i as f64)],
                    Vec::new(),
                )
                .expect("解释调用");
            results.push(v);
        }
        assert!(
            matches!(vm.jit_slots[dbl_idx], JitSlot::Cold),
            "阈值前应仍为 Cold，实际 {:?}",
            vm.jit_slots[dbl_idx]
        );

        // 跨阈值：应编译并以机器码执行，结果与解释器逐位一致
        for i in 0..(JIT_HOT_THRESHOLD - 1) {
            let v = vm
                .invoke_function(
                    dbl_idx,
                    Value::Undefined,
                    &[Value::Number(i as f64)],
                    Vec::new(),
                )
                .expect("JIT 调用");
            let (Some(a), Some(b)) = (v.as_number(), results[i as usize].as_number()) else {
                panic!("均应为数值")
            };
            assert_eq!(
                a.to_bits(),
                b.to_bits(),
                "第 {i} 次：JIT 与解释器须逐位一致"
            );
        }
        assert!(
            matches!(vm.jit_slots[dbl_idx], JitSlot::Compiled(_)),
            "跨阈值后应为 Compiled，实际 {:?}",
            vm.jit_slots[dbl_idx]
        );
    }

    /// 资格判定：生成器/async/varargs/try 一律拒绝编译。
    #[test]
    fn ineligible_templates_are_rejected() {
        let base = double_plus_one();
        let mut generator = base.clone();
        generator.is_generator = true;
        assert!(!is_jit_eligible(&generator, false), "生成器不得编译");
        let mut asy = base.clone();
        asy.is_async = true;
        assert!(!is_jit_eligible(&asy, false), "async 不得编译");
        let mut var = base.clone();
        var.is_var_args = true;
        assert!(!is_jit_eligible(&var, false), "varargs 不得编译");
        let mut tr = base.clone();
        tr.try_table = vec![aluka_bytecode::TryEntry {
            start_pc: 0,
            catch_pc: 4,
            finally_pc: 0,
            has_catch: true,
            has_finally: false,
            end_pc: 4,
            catch_end_pc: 8,
            finally_end_pc: 0,
        }];
        assert!(!is_jit_eligible(&tr, false), "含 try 表不得编译");
        assert!(!is_jit_eligible(&base, true), "需 arguments 对象不得编译");
        assert!(is_jit_eligible(&base, false), "纯数值函数应具备资格");
    }

    /// 子集外函数（含 Call）计数达阈值后转 `Rejected`，且不再重试。
    #[test]
    fn unsupported_opcode_marks_rejected_once() {
        let mut callee = double_plus_one();
        // 插入一个子集外操作码（TypeOf）使编译失败
        callee.code.insert(0, Instr::new(Op::Typeof, 0));
        callee.code.insert(0, Instr::new(Op::PushUndefined, 0));
        let module = BytecodeModule {
            header_extras: Vec::new(),
            version: 30,
            functions: vec![caller(0), callee],
            classes: Vec::new(),
        };
        let mut vm = Vm::new(0);
        vm.load_module_for_test(&module);
        for _ in 0..(JIT_HOT_THRESHOLD + 5) {
            let _ = vm.invoke_function(1, Value::Undefined, &[Value::Number(1.0)], Vec::new());
        }
        assert!(
            matches!(vm.jit_slots[1], JitSlot::Rejected),
            "子集外函数应标记 Rejected，实际 {:?}",
            vm.jit_slots[1]
        );
    }
}

#[cfg(test)]
mod call_chain_tests {
    use super::*;
    use aluka_bytecode::{BytecodeModule, Constant, Instr, Op};

    /// JIT 内 `CALL` 的被调函数自身也应升级为机器码（计数跨阈值后）。
    #[test]
    fn callee_invoked_from_jit_also_gets_compiled() {
        let caller = FuncTemplate {
            name: "caller".to_owned(),
            num_params: 0,
            num_locals: 1,
            is_var_args: false,
            is_generator: false,
            is_async: false,
            is_arrow: false,
            code: vec![
                Instr::new(Op::LoadGlobal, 0),
                Instr::new(Op::PushConst, 1),
                Instr::new(Op::Call, 1),
                Instr::new(Op::Return, 0),
            ],
            max_stack: 8,
            source_file: String::new(),
            constants: vec![Constant::String("cb".to_owned()), Constant::Number(3.0)],
            upvalues: Vec::new(),
            try_table: Vec::new(),
            line_table: Vec::new(),
        };
        let callee = FuncTemplate {
            name: "cb".to_owned(),
            num_params: 1,
            num_locals: 2,
            is_var_args: false,
            is_generator: false,
            is_async: false,
            is_arrow: false,
            code: vec![
                Instr::new(Op::LoadLocal, 1),
                Instr::new(Op::PushConst, 0),
                Instr::new(Op::Mul, 0),
                Instr::new(Op::Return, 0),
            ],
            max_stack: 8,
            source_file: String::new(),
            constants: vec![Constant::Number(2.0)],
            upvalues: Vec::new(),
            try_table: Vec::new(),
            line_table: Vec::new(),
        };
        let module = BytecodeModule {
            header_extras: Vec::new(),
            version: 30,
            functions: vec![caller.clone(), callee],
            classes: Vec::new(),
        };
        let mut vm = Vm::new(0);
        vm.load_module_for_test(&module);
        let closure = vm.alloc_closure(1);
        vm.globals.insert("cb".to_owned(), Value::Object(closure));

        // 反复调用 caller（func 0）：caller 与 cb（func 1）都应跨阈值
        for _ in 0..(JIT_HOT_THRESHOLD * 2) {
            let v = vm
                .invoke_function(0, Value::Undefined, &[], Vec::new())
                .expect("调用 caller");
            let Some(n) = v.as_number() else {
                panic!("应为数值")
            };
            assert_eq!(n, 6.0, "cb(3) = 3*2 = 6");
        }
        assert!(
            matches!(vm.jit_slots[0], JitSlot::Compiled(_)),
            "caller 应已编译，实际 {:?}",
            vm.jit_slots[0]
        );
        assert!(
            matches!(vm.jit_slots[1], JitSlot::Compiled(_)),
            "经 JIT 的 CALL 调用的 cb 也应升级为机器码，实际 {:?}",
            vm.jit_slots[1]
        );
    }
}
