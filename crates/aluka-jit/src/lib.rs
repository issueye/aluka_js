//! M5 JIT（J2 阶段，ADR 0005）：字节码数值子集 → Cranelift 机器码。
//!
//! 值域升级为 NaN-box u64（见 [`valbox`]），属性访问经 [`ctx`] 桥回调
//! 解释器 helper（J2 阶段先全回退，PIC 快速路径见后续阶段）。
//!
//! 子集操作码：`PushConst/PushInt/PushNegInt/PushTrue/PushFalse/PushUndefined /
//! LoadLocal/StoreLocal/Dup/Swap/Add/Sub/Mul/Div/Lt/Le/Gt/Ge/Eq/Ne/
//! StrictEq/StrictNe/Jmp/JmpTruePop/JmpFalsePop/JmpTrueKeep/JmpFalseKeep/
//! Inc/Dec/Neg/Not/Return/Pop/Nop`。数值运算走快速路径（`ToNumber` 后 f64
//! 运算），非数值强制转换经 `ctx.vtable` helper 回调解释器，语义与 Tier 0
//! 逐位一致。
//!
//! 函数签名 `(ctx, args_ptr, len) -> u64`（u64 为 NaN-box 值）。
//! 子集外操作码（调用/闭包/生成器/Try 等）→ 编译期拒绝
//! （[`JitError::UnsupportedOpcode`]），调用方回退解释器。

pub mod ctx;
pub mod peephole;
pub mod valbox;

use crate::ctx::{CallCell, GlobalCell, JitCtx, JitLayout, JitVtable, PicCell};
use crate::valbox::*;
use aluka_bytecode::{Constant, FuncTemplate, Op};
use cranelift::codegen::ir;
use cranelift::codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift::prelude::*;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::DataDescription;
use cranelift_module::{Linkage, Module};
use std::collections::{BTreeMap, BTreeSet};

/// `JitCtx` 内堆/布局字段偏移（repr(C) 稳定）。
const CTX_CONSTS_PTR_OFF: i32 = std::mem::offset_of!(JitCtx, consts_ptr) as i32;
const CTX_CONSTS_LEN_OFF: i32 = std::mem::offset_of!(JitCtx, consts_len) as i32;
const CTX_GLOBALS_GEN_OFF: i32 = std::mem::offset_of!(JitCtx, globals_gen) as i32;
const CTX_HEAP_PTR_OFF: i32 = std::mem::offset_of!(JitCtx, heap_ptr) as i32;
const CTX_HEAP_STRIDE_OFF: i32 = std::mem::offset_of!(JitCtx, heap_stride) as i32;
const CTX_FRAMES_PTR_OFF: i32 = std::mem::offset_of!(JitCtx, frames_ptr) as i32;
const CTX_JIT_GEN_OFF: i32 = std::mem::offset_of!(JitCtx, jit_gen) as i32;
const CTX_LAYOUT_OFF: i32 = std::mem::offset_of!(JitCtx, layout) as i32;
/// `JitLayout` 内字段偏移。
const LAYOUT_PROPS_OFF: i32 = std::mem::offset_of!(JitLayout, props_off) as i32;
const LAYOUT_SHAPE_ID_OFF: i32 = std::mem::offset_of!(JitLayout, shape_id_off) as i32;
const LAYOUT_SLOTS_PTR_OFF: i32 = std::mem::offset_of!(JitLayout, slots_ptr_off) as i32;
const LAYOUT_SLOTS_DATA_OFF: i32 = std::mem::offset_of!(JitLayout, slots_data_off) as i32;
const LAYOUT_DELETED_GEN_OFF: i32 = std::mem::offset_of!(JitLayout, deleted_gen_off) as i32;
const LAYOUT_HAS_ACCESSORS_OFF: i32 = std::mem::offset_of!(JitLayout, has_accessors_off) as i32;
const LAYOUT_DISC_SHAPE: i32 = std::mem::offset_of!(JitLayout, disc_shape) as i32;
/// `PicCell` 字段偏移。
const PIC_SHAPE_ID: i32 = std::mem::offset_of!(PicCell, shape_id) as i32;
const PIC_SLOT: i32 = std::mem::offset_of!(PicCell, slot) as i32;
const PIC_STATE: i32 = std::mem::offset_of!(PicCell, state) as i32;
/// `CallCell` 字段偏移（JIT→JIT 原生直调守卫）。
const CALL_CALLEE: i32 = std::mem::offset_of!(CallCell, callee) as i32;
const CALL_ENTRY: i32 = std::mem::offset_of!(CallCell, entry) as i32;
const CALL_CONSTS_PTR: i32 = std::mem::offset_of!(CallCell, consts_ptr) as i32;
const CALL_CONSTS_LEN: i32 = std::mem::offset_of!(CallCell, consts_len) as i32;
const CALL_GEN: i32 = std::mem::offset_of!(CallCell, cell_gen) as i32;
const CALL_STATE: i32 = std::mem::offset_of!(CallCell, state) as i32;
/// `GlobalCell` 字段偏移。
const GLOBAL_KEY_IDX: i32 = std::mem::offset_of!(GlobalCell, key_idx) as i32;
const GLOBAL_STATE: i32 = std::mem::offset_of!(GlobalCell, state) as i32;
const GLOBAL_GEN: i32 = std::mem::offset_of!(GlobalCell, globals_gen) as i32;
const GLOBAL_VALUE: i32 = std::mem::offset_of!(GlobalCell, value) as i32;

/// helper 导入符号名（经 `symbol_lookup` 解析到 VM 的 `extern "C"` 函数）。
pub(crate) const HELPER_GET_PROPERTY: &str = "aluka_jit.get_property";
pub(crate) const HELPER_SET_PROPERTY: &str = "aluka_jit.set_property";
pub(crate) const HELPER_ALLOC_ORDINARY: &str = "aluka_jit.alloc_ordinary";
pub(crate) const HELPER_ADD: &str = "aluka_jit.add";
pub(crate) const HELPER_EQ: &str = "aluka_jit.eq";
pub(crate) const HELPER_STRICT_EQ: &str = "aluka_jit.strict_eq";
pub(crate) const HELPER_TO_NUMBER: &str = "aluka_jit.to_number";
pub(crate) const HELPER_TO_BOOLEAN: &str = "aluka_jit.to_boolean";
pub(crate) const HELPER_CALL: &str = "aluka_jit.call";
pub(crate) const HELPER_CALL_METHOD: &str = "aluka_jit.call_method";
pub(crate) const HELPER_LOAD_GLOBAL: &str = "aluka_jit.load_global";
pub(crate) const HELPER_LOAD_UPVALUE: &str = "aluka_jit.load_upvalue";

/// JIT 编译错误。
#[derive(Debug)]
pub enum JitError {
    /// 子集外操作码（调用方回退解释器）
    UnsupportedOpcode {
        /// 函数名（含诊断信息）
        func: String,
        /// 折叠后指令流中的位置
        pc: usize,
    },
    /// Cranelift 编译失败
    Codegen(String),
}

/// 已编译的可执行函数：`(ctx, args_ptr, len) -> u64`。
pub struct JittedFn {
    ptr: *const u8,
    /// 持有以保活已 finalize 的代码内存
    #[allow(dead_code)]
    module: JITModule,
    /// peephole 折叠后的指令数（诊断用）
    pub folded_len: usize,
    /// 编译产物是否读取上值（`LOAD_UPVALUE`）。
    ///
    /// 读上值的函数依赖调用方安装 `current_upvalues`，因此**不可作为
    /// JIT→JIT 原生直调的被调**（直调不经解释器帧、不换上值表）。
    pub uses_upvalues: bool,
}

// SAFETY: `ptr` 指向已 finalize 的只读可执行代码，其生命周期由同结构体
// 持有的 `module` 保证；代码页不含线程局部状态、执行期不改写自身，因此
// 跨线程转移所有权后调用仍安全。
unsafe impl Send for JittedFn {}

impl JittedFn {
    /// 机器码入口地址（供 [`ctx::CallCell`] 登记，JIT 侧 `call_indirect` 目标）。
    ///
    /// 地址在 `self` 存活期间有效（代码内存由 `self.module` 保活）；调用方
    /// 必须保证登记入口的缓存单元不长于本对象的生命周期——VM 侧以
    /// `jit_gen`（模块函数表替换即递增）作守卫。
    #[must_use]
    pub fn entry_addr(&self) -> usize {
        self.ptr as usize
    }

    /// 执行：按新 ABI 调用机器码（完整值域）。
    ///
    /// # Safety 论证（SAFETY）
    /// `self.ptr` 指向 Cranelift `finalize_definitions` 之后的可执行代码，
    /// 生命周期由 `self.module` 保活；签名 `(ctx, args_ptr, usize) -> u64` 与
    /// 编译期声明的 Cranelift 签名逐字段一致（指针 + 指针 + I64 长度，返回
    /// I64）。`ctx` 由调用方构造（VM 填充 vm/常量池/vtable），helper 经
    /// `ctx.vm` 同步回调解释器，无跨线程别名（单线程契约）。
    pub fn call_ctx(&self, ctx: &mut JitCtx, args: &[u64]) -> u64 {
        // SAFETY: 见方法级论证——指针有效性与签名一致性由 JITModule 保证
        let f = unsafe {
            std::mem::transmute::<*const u8, fn(*mut JitCtx, *const u64, usize) -> u64>(self.ptr)
        };
        f(ctx, args.as_ptr(), args.len())
    }

    /// 数值便捷入口：实参按 `Number` 入盒；结果按数值出盒，布尔盒映射为
    /// 1.0/0.0（对齐 J1 的 f64 语义），其余非数值返回 NaN（纯数值契约）。
    ///
    /// 内部使用零值 [`ctx::zeroed_ctx`]：仅当编译产物既不读 ctx 字段也不调用
    /// helper（纯数值计算的快速路径全命中）时安全；非数值语义请走
    /// [`Self::call_ctx`]（测试与基准应优先构造真实 ctx）。
    pub fn call(&self, args: &[f64]) -> f64 {
        let mut ctx = ctx::zeroed_ctx();
        let boxes: Vec<u64> = args.iter().map(|&n| box_number(n)).collect();
        let r = self.call_ctx(&mut ctx, &boxes);
        if is_number(r) {
            unbox_number(r)
        } else if r == TRUE {
            1.0
        } else if r == FALSE {
            0.0
        } else {
            f64::NAN
        }
    }
}

/// Cranelift 代码生成状态。
struct Cg<'f> {
    fb: FunctionBuilder<'f>,
    /// 当前正在填充的块
    current: Block,
    /// 临时 Variable 分配计数器（避开局部槽位 id 区间）
    var_next: u32,
    /// 已知承载 `TRUE`/`FALSE` 盒的 IR 值（比较与逻辑运算的产物）。
    ///
    /// 真值判定的快速路径只覆盖数值域，布尔盒会掉进 `to_boolean` helper——
    /// 而 `while (i > 0)` 这类条件**每轮**都在判定一个比较结果，等于每次迭代
    /// 跨一次 FFI 边界。登记这些值后，真值判定退化为一条 `icmp`。
    bool_boxes: std::collections::HashSet<Value>,
}

impl<'f> Cg<'f> {
    /// 分配一个临时 Variable（类型 `I64`）。
    fn new_var(&mut self) -> Variable {
        let v = Variable::from_u32(self.var_next);
        self.var_next += 1;
        self.fb.declare_var(v, types::I64);
        v
    }

    /// `v` 是否为数值盒（I8 布尔）。
    fn is_number(&mut self, v: Value) -> Value {
        let masked = self.fb.ins().band_imm(v, TAG_MASK as i64);
        let tag = self.fb.ins().iconst(types::I64, TAG_PREFIX as i64);
        self.fb.ins().icmp(IntCC::NotEqual, masked, tag)
    }

    /// 「两个操作数都是数值盒」的判定，编译期已知常量的一侧直接省掉判型。
    fn both_numbers(&mut self, a: Value, b: Value) -> Option<Value> {
        match (known_number(self, a), known_number(self, b)) {
            (true, true) => None, // 两侧都是常量数值 → 无需运行时判定
            (true, false) => Some(self.is_number(b)),
            (false, true) => Some(self.is_number(a)),
            (false, false) => {
                let na = self.is_number(a);
                let nb = self.is_number(b);
                Some(self.fb.ins().band(na, nb))
            }
        }
    }

    /// 数值盒出盒为 f64（调用方须已 guard 数值）。
    fn unbox(&mut self, v: Value) -> Value {
        self.fb.ins().bitcast(types::F64, MemFlags::new(), v)
    }

    /// f64 入盒（NaN 规范化到 tag 空间外）。
    fn boxed(&mut self, r: Value) -> Value {
        let bits = self.fb.ins().bitcast(types::I64, MemFlags::new(), r);
        let is_nan = self.fb.ins().fcmp(FloatCC::Unordered, r, r);
        let canon = self.fb.ins().iconst(types::I64, NAN_CANONICAL as i64);
        self.fb.ins().select(is_nan, canon, bits)
    }

    /// 数值域内的真值判定（非零且非 NaN），返回布尔。
    fn fast_truthy(&mut self, v: Value) -> Value {
        let n = self.unbox(v);
        let zero = self.fb.ins().f64const(0.0);
        let ne0 = self.fb.ins().fcmp(FloatCC::NotEqual, n, zero);
        let ord = self.fb.ins().fcmp(FloatCC::Ordered, n, n);
        self.fb.ins().band(ne0, ord)
    }

    /// 无条件跳转到 `target`。
    fn jump_to(&mut self, target: Block) {
        self.fb.ins().jump(target, &[]);
    }

    /// 切换到块。块一律不在中途封口——回边（循环体跳回已遍历的块）在
    /// 封口后才发射会触发 `declare_block_predecessor` 断言；统一在编译末尾
    /// `seal_all_blocks`（frontend 对未封口块做延迟变量解析，见 J1 同款做法）。
    fn goto(&mut self, b: Block) {
        self.fb.switch_to_block(b);
        self.current = b;
    }

    /// 布尔盒 → 是否等于 TRUE（I8 比较）。
    fn box_is_true(&mut self, v: Value) -> Value {
        self.fb.ins().icmp_imm(IntCC::Equal, v, TRUE as i64)
    }

    /// `v` 是否为对象盒（I1）。
    fn is_object(&mut self, v: Value) -> Value {
        let tag = self.fb.ins().band_imm(v, TAG_MASK as i64);
        let prefix = self.fb.ins().iconst(types::I64, TAG_PREFIX as i64);
        let tag_ok = self.fb.ins().icmp(IntCC::Equal, tag, prefix);
        let low = self.fb.ins().band_imm(v, 0xFF);
        let obj_tag = self.fb.ins().iconst(types::I64, TAG_OBJECT as i64);
        let low_ok = self.fb.ins().icmp(IntCC::Equal, low, obj_tag);
        self.fb.ins().band(tag_ok, low_ok)
    }

    /// 对象盒出盒为 ObjectRef（I64，调用方须已 guard 对象）。
    fn unbox_object_i64(&mut self, b: Value) -> Value {
        let r = self.fb.ins().ushr_imm(b, 8);
        self.fb.ins().band_imm(r, 0xFFFF_FFFF)
    }

    /// 加载 ctx.layout 内的布局偏移（I64 运行时值）。
    ///
    /// `JitLayout` 内嵌在 `JitCtx` 中（非指针）：以 `ctx + CTX_LAYOUT_OFF` 为
    /// 基址做指针运算后按字段偏移读取。标 `readonly`——布局与堆步长在整个
    /// 调用期间恒定（进程内实测一次后写入 ctx），让 GVN/LICM 把循环里每轮
    /// 重复的这些加载提到循环外。
    fn layout_field(&mut self, ctx_val: Value, foff: i32) -> Value {
        let base = self.fb.ins().iadd_imm(ctx_val, CTX_LAYOUT_OFF as i64);
        self.fb
            .ins()
            .load(types::I64, immutable_flags(), base, foff)
    }

    /// 活跃 JIT 帧计数 `+= delta`（内联的内存读改写，不经 helper）。
    ///
    /// 入口 +1、每个返回点 −1，令 VM 侧在 JIT 帧内跳过 GC——机器码的局部
    /// 无栈映射，GC 扫不到（见 `JitCtx::frames_ptr` 与 `Vm::push_object`）。
    fn bump_frames(&mut self, ctx_val: Value, ptr_type: ir::Type, delta: i64) {
        let p = self
            .fb
            .ins()
            .load(ptr_type, immutable_flags(), ctx_val, CTX_FRAMES_PTR_OFF);
        let cur = self.fb.ins().load(types::I32, MemFlags::new(), p, 0);
        let next = self.fb.ins().iadd_imm(cur, delta);
        self.fb.ins().store(MemFlags::new(), next, p, 0);
    }
}

/// 将常量下标装入 I32 参数值。
fn key_value(cg: &mut Cg, key_idx: u32) -> Value {
    cg.fb.ins().iconst(types::I32, key_idx as i64)
}

/// 「调用期间不变」的加载标记：`ctx.layout` 与 `ctx.heap_stride` 在一次 JIT
/// 调用内恒定（进程内实测一次即固定），标 `readonly` 让 Cranelift 的 GVN/LICM
/// 把循环体里每轮重复的这些加载提出去。
///
/// **不可用于** `heap_ptr` / `consts_ptr` / PIC 单元 / 槽位——它们会被 helper
/// 或直调改写（`Vec` 扩容、常量池换入、缓存回写），提出循环即读到过期值。
fn immutable_flags() -> MemFlags {
    let mut f = MemFlags::new();
    f.set_readonly();
    f
}

/// 经模块 Import 调用 helper：`call(fref, [ctx, args…])`。
///
/// 走 Cranelift 的外部调用路径（符号经 `symbol_lookup` 解析到 VM 的
/// `extern "C"` 函数），调用序列由代码生成器按目标 ABI 正确发射——比裸
/// `call_indirect` 可靠（Windows x64 的实参寄存器/栈对齐由后端保证）。
fn emit_helper(cg: &mut Cg, fref: ir::FuncRef, ctx_val: Value, args: &[Value]) -> Value {
    let mut all = Vec::with_capacity(args.len() + 1);
    all.push(ctx_val);
    all.extend_from_slice(args);
    let inst = cg.fb.ins().call(fref, &all);
    cg.fb.inst_results(inst)[0]
}

/// 双路汇合：`cond` 为真走 fast，否则 slow，结果经 Variable 汇入 join。
fn branch_join(
    cg: &mut Cg,
    cond: Value,
    fast: impl FnOnce(&mut Cg) -> Value,
    slow: impl FnOnce(&mut Cg) -> Value,
) -> Value {
    let fast_b = cg.fb.create_block();
    let slow_b = cg.fb.create_block();
    let join_b = cg.fb.create_block();
    let v = cg.new_var();
    cg.fb.ins().brif(cond, fast_b, &[], slow_b, &[]);

    cg.goto(fast_b);
    let r1 = fast(cg);
    cg.fb.def_var(v, r1);
    cg.jump_to(join_b);

    cg.goto(slow_b);
    let r2 = slow(cg);
    cg.fb.def_var(v, r2);
    cg.jump_to(join_b);

    cg.goto(join_b);
    cg.fb.use_var(v)
}

/// 双路汇合（布尔版，真值判定用）。
fn branch_join_bool(
    cg: &mut Cg,
    cond: Value,
    fast: impl FnOnce(&mut Cg) -> Value,
    slow: impl FnOnce(&mut Cg) -> Value,
) -> Value {
    let fast_b = cg.fb.create_block();
    let slow_b = cg.fb.create_block();
    let join_b = cg.fb.create_block();
    let v = Variable::from_u32(cg.var_next);
    cg.var_next += 1;
    cg.fb.declare_var(v, types::I8);
    cg.fb.ins().brif(cond, fast_b, &[], slow_b, &[]);

    cg.goto(fast_b);
    let r1 = fast(cg);
    cg.fb.def_var(v, r1);
    cg.jump_to(join_b);

    cg.goto(slow_b);
    let r2 = slow(cg);
    cg.fb.def_var(v, r2);
    cg.jump_to(join_b);

    cg.goto(join_b);
    cg.fb.use_var(v)
}

/// `ADD`：数值快速路径 + `add_values` helper 回退。
fn emit_add(cg: &mut Cg, ctx_val: Value, a: Value, b: Value, fref_add: ir::FuncRef) -> Value {
    let add_fast = |cg: &mut Cg| {
        let af = cg.unbox(a);
        let bf = cg.unbox(b);
        let r = cg.fb.ins().fadd(af, bf);
        cg.boxed(r)
    };
    match cg.both_numbers(a, b) {
        // 两侧编译期已知为数值：直接算，不发判型分支
        None => add_fast(cg),
        Some(both) => branch_join(cg, both, add_fast, |cg| {
            emit_helper(cg, fref_add, ctx_val, &[a, b])
        }),
    }
}

/// 该 IR 值是否为**编译期已知的数值盒**（由 `iconst` 定义且落在数值域）。
///
/// 子集里绝大多数二元运算有一侧是常量（`i > 0`、`i - 1`、`n * 1.5`），
/// 对这些操作数发射 `is_number` 判型 + 双路汇合纯属浪费：分支、Variable
/// 汇入、以及后续 GVN 无法跨越的合流点都省不掉。命中即直接当数值用。
fn known_number(cg: &Cg, v: Value) -> bool {
    let ir::ValueDef::Result(inst, _) = cg.fb.func.dfg.value_def(v) else {
        return false;
    };
    match cg.fb.func.dfg.insts[inst] {
        ir::InstructionData::UnaryImm {
            opcode: ir::Opcode::Iconst,
            imm,
        } => is_number(imm.bits() as u64),
        _ => false,
    }
}

/// 强制转数值：数值直通，否则 `to_number` helper。
fn emit_to_number(cg: &mut Cg, ctx_val: Value, a: Value, fref_tonum: ir::FuncRef) -> Value {
    if known_number(cg, a) {
        return a;
    }
    let is_num = cg.is_number(a);
    branch_join(
        cg,
        is_num,
        |_cg| a,
        |cg| emit_helper(cg, fref_tonum, ctx_val, &[a]),
    )
}

/// 真值判定：数值域内快速，否则 `to_boolean` helper；返回布尔。
fn emit_truthy(cg: &mut Cg, ctx_val: Value, a: Value, fref_tobool: ir::FuncRef) -> Value {
    // 已知为布尔盒（比较/逻辑运算的产物）：真值性就是「等于 TRUE」
    if cg.bool_boxes.contains(&a) {
        return cg.box_is_true(a);
    }
    if known_number(cg, a) {
        return cg.fast_truthy(a);
    }
    let is_num = cg.is_number(a);
    branch_join_bool(
        cg,
        is_num,
        |cg| cg.fast_truthy(a),
        |cg| {
            let b = emit_helper(cg, fref_tobool, ctx_val, &[a]);
            cg.box_is_true(b)
        },
    )
}

/// 算术（Sub/Mul/Div）：ToNumber 后 f64 运算。
fn emit_arith(
    cg: &mut Cg,
    ctx_val: Value,
    a: Value,
    b: Value,
    op: Op,
    fref_tonum: ir::FuncRef,
) -> Value {
    let na = emit_to_number(cg, ctx_val, a, fref_tonum);
    let nb = emit_to_number(cg, ctx_val, b, fref_tonum);
    let af = cg.unbox(na);
    let bf = cg.unbox(nb);
    let r = match op {
        Op::Sub => cg.fb.ins().fsub(af, bf),
        Op::Mul => cg.fb.ins().fmul(af, bf),
        Op::Div => cg.fb.ins().fdiv(af, bf),
        _ => unreachable!("emit_arith 仅接受二元算术"),
    };
    cg.boxed(r)
}

/// 数值比较（Lt/Le/Gt/Ge）：ToNumber 后 fcmp。产物为布尔盒。
fn emit_cmp(
    cg: &mut Cg,
    ctx_val: Value,
    a: Value,
    b: Value,
    op: Op,
    fref_tonum: ir::FuncRef,
) -> Value {
    let na = emit_to_number(cg, ctx_val, a, fref_tonum);
    let nb = emit_to_number(cg, ctx_val, b, fref_tonum);
    let af = cg.unbox(na);
    let bf = cg.unbox(nb);
    let cc = match op {
        Op::Lt => FloatCC::LessThan,
        Op::Le => FloatCC::LessThanOrEqual,
        Op::Gt => FloatCC::GreaterThan,
        _ => FloatCC::GreaterThanOrEqual,
    };
    let cmp = cg.fb.ins().fcmp(cc, af, bf);
    let t = cg.fb.ins().iconst(types::I64, TRUE as i64);
    let f = cg.fb.ins().iconst(types::I64, FALSE as i64);
    let r = cg.fb.ins().select(cmp, t, f);
    cg.bool_boxes.insert(r);
    r
}

/// 相等系（Eq/Ne/StrictEq/StrictNe）：数值快速路径 + helper 回退。
/// 产物一律为布尔盒。
fn emit_eq(
    cg: &mut Cg,
    ctx_val: Value,
    a: Value,
    b: Value,
    op: Op,
    fref_eq: ir::FuncRef,
    fref_strict_eq: ir::FuncRef,
) -> Value {
    let both = cg.both_numbers(a, b);
    let flip = cg.fb.ins().iconst(types::I64, (TRUE ^ FALSE) as i64);
    // 数值域内的相等（含 Ne 取反）；TRUE/FALSE tag 仅差 bit0，异或 1 即取反
    let num_eq = |cg: &mut Cg| {
        let af = cg.unbox(a);
        let bf = cg.unbox(b);
        let cmp = cg.fb.ins().fcmp(FloatCC::Equal, af, bf);
        let t = cg.fb.ins().iconst(types::I64, TRUE as i64);
        let f = cg.fb.ins().iconst(types::I64, FALSE as i64);
        let r = cg.fb.ins().select(cmp, t, f);
        if matches!(op, Op::Ne | Op::StrictNe) {
            cg.fb.ins().bxor(r, flip)
        } else {
            r
        }
    };
    let Some(both) = both else {
        // 两侧编译期已知为数值：直接比较，不发判型分支
        let r = num_eq(cg);
        cg.bool_boxes.insert(r);
        return r;
    };
    let fast_b = cg.fb.create_block();
    let slow_b = cg.fb.create_block();
    let join_b = cg.fb.create_block();
    let v = cg.new_var();
    cg.fb.ins().brif(both, fast_b, &[], slow_b, &[]);

    cg.goto(fast_b);
    let res = num_eq(cg);
    cg.fb.def_var(v, res);
    cg.jump_to(join_b);

    cg.goto(slow_b);
    let fref = match op {
        Op::StrictEq | Op::StrictNe => fref_strict_eq,
        _ => fref_eq,
    };
    let mut h = emit_helper(cg, fref, ctx_val, &[a, b]);
    if matches!(op, Op::Ne | Op::StrictNe) {
        h = cg.fb.ins().bxor(h, flip);
    }
    cg.fb.def_var(v, h);
    cg.jump_to(join_b);

    cg.goto(join_b);
    let r = cg.fb.use_var(v);
    cg.bool_boxes.insert(r);
    r
}

/// `LOAD_GLOBAL`：Global IC 命中直读盒，失配走 helper（helper 负责回写）。
fn emit_load_global(
    cg: &mut Cg,
    ctx_val: Value,
    key_idx: Value,
    cell_addr: Value,
    fref_global: ir::FuncRef,
) -> Value {
    let fast_b = cg.fb.create_block();
    let slow_b = cg.fb.create_block();
    let join_b = cg.fb.create_block();
    let res = cg.new_var();
    let state = cg
        .fb
        .ins()
        .load(types::I32, MemFlags::new(), cell_addr, GLOBAL_STATE);
    let cached_key = cg
        .fb
        .ins()
        .load(types::I32, MemFlags::new(), cell_addr, GLOBAL_KEY_IDX);
    let cached_gen = cg
        .fb
        .ins()
        .load(types::I32, MemFlags::new(), cell_addr, GLOBAL_GEN);
    let current_gen = cg
        .fb
        .ins()
        .load(types::I32, immutable_flags(), ctx_val, CTX_GLOBALS_GEN_OFF);
    let state_ok = cg
        .fb
        .ins()
        .icmp_imm(IntCC::Equal, state, GlobalCell::FAST as i64);
    let key_ok = cg.fb.ins().icmp(IntCC::Equal, cached_key, key_idx);
    let gen_ok = cg.fb.ins().icmp(IntCC::Equal, cached_gen, current_gen);
    let ok = cg.fb.ins().band(state_ok, key_ok);
    let ok = cg.fb.ins().band(ok, gen_ok);
    cg.fb.ins().brif(ok, fast_b, &[], slow_b, &[]);

    cg.goto(fast_b);
    let value = cg
        .fb
        .ins()
        .load(types::I64, MemFlags::new(), cell_addr, GLOBAL_VALUE);
    cg.fb.def_var(res, value);
    cg.jump_to(join_b);

    cg.goto(slow_b);
    let value = emit_helper(cg, fref_global, ctx_val, &[key_idx, cell_addr]);
    cg.fb.def_var(res, value);
    cg.jump_to(join_b);

    cg.goto(join_b);
    cg.fb.use_var(res)
}

/// 属性读取 PIC：守卫链（对象盒 → Shape 模式 → 无删除/无访问器 →
/// shape 命中缓存）命中则槽位直读，否则 helper 全语义回退并回写缓存。
fn emit_get_prop(
    cg: &mut Cg,
    ctx_val: Value,
    obj_box: Value,
    key_idx: Value,
    cell_addr: Value,
    fref_get: ir::FuncRef,
) -> Value {
    let slow_b = cg.fb.create_block();
    let join_b = cg.fb.create_block();
    let res = cg.new_var();

    // 1. 非对象盒 → slow
    let is_obj = cg.is_object(obj_box);
    let obj_ok_b = cg.fb.create_block();
    cg.fb.ins().brif(is_obj, obj_ok_b, &[], slow_b, &[]);
    cg.goto(obj_ok_b);

    // 对象地址 = heap + ref*stride
    let refv = cg.unbox_object_i64(obj_box);
    let heap = cg
        .fb
        .ins()
        .load(types::I64, MemFlags::new(), ctx_val, CTX_HEAP_PTR_OFF);
    let stride = cg
        .fb
        .ins()
        .load(types::I64, immutable_flags(), ctx_val, CTX_HEAP_STRIDE_OFF);
    let byte = cg.fb.ins().imul(refv, stride);
    let obj_addr = cg.fb.ins().iadd(heap, byte);

    // 布局偏移（运行时来自 ctx.layout；判别式是值不是偏移，直接读）
    let props_off = cg.layout_field(ctx_val, LAYOUT_PROPS_OFF);
    let layout_base = cg.fb.ins().iadd_imm(ctx_val, CTX_LAYOUT_OFF as i64);
    let disc_shape = cg.fb.ins().load(
        types::I32,
        immutable_flags(),
        layout_base,
        LAYOUT_DISC_SHAPE,
    );
    let deleted_gen_off = cg.layout_field(ctx_val, LAYOUT_DELETED_GEN_OFF);
    let has_acc_off = cg.layout_field(ctx_val, LAYOUT_HAS_ACCESSORS_OFF);

    let props_addr = cg.fb.ins().iadd(obj_addr, props_off);
    let props = cg.fb.ins().load(types::I32, MemFlags::new(), props_addr, 0);
    let dg_addr = cg.fb.ins().iadd(obj_addr, deleted_gen_off);
    let dg = cg.fb.ins().load(types::I32, MemFlags::new(), dg_addr, 0);
    let ha_addr = cg.fb.ins().iadd(obj_addr, has_acc_off);
    let ha = cg.fb.ins().load(types::I32, MemFlags::new(), ha_addr, 0);

    let is_shape = cg.fb.ins().icmp(IntCC::Equal, props, disc_shape);
    let no_del = cg.fb.ins().icmp_imm(IntCC::Equal, dg, 0);
    let no_acc = cg.fb.ins().icmp_imm(IntCC::Equal, ha, 0);
    let fast1 = cg.fb.ins().band(is_shape, no_del);
    let fast1 = cg.fb.ins().band(fast1, no_acc);

    // 2. shape 命中缓存
    // shape_id/slots 偏移相对 props 变体基址 → 须叠加 props_off
    let shape_id_off = cg.layout_field(ctx_val, LAYOUT_SHAPE_ID_OFF);
    let shape_chain = cg.fb.ins().iadd(props_off, shape_id_off);
    let shape_addr = cg.fb.ins().iadd(obj_addr, shape_chain);
    let shape = cg.fb.ins().load(types::I32, MemFlags::new(), shape_addr, 0);
    let state = cg
        .fb
        .ins()
        .load(types::I32, MemFlags::new(), cell_addr, PIC_STATE);
    let cached = cg
        .fb
        .ins()
        .load(types::I32, MemFlags::new(), cell_addr, PIC_SHAPE_ID);
    let state_ok = cg
        .fb
        .ins()
        .icmp_imm(IntCC::Equal, state, PicCell::FAST as i64);
    let shape_ok = cg.fb.ins().icmp(IntCC::Equal, shape, cached);
    let fast2 = cg.fb.ins().band(state_ok, shape_ok);
    let fast = cg.fb.ins().band(fast1, fast2);

    let fast_b = cg.fb.create_block();
    cg.fb.ins().brif(fast, fast_b, &[], slow_b, &[]);
    cg.goto(fast_b);

    // 3. 槽位直读（槽位为 u64 盒）
    let slot = cg
        .fb
        .ins()
        .load(types::I32, MemFlags::new(), cell_addr, PIC_SLOT);
    let slot64 = cg.fb.ins().uextend(types::I64, slot);
    let slots_ptr_off = cg.layout_field(ctx_val, LAYOUT_SLOTS_PTR_OFF);
    let slots_data_off = cg.layout_field(ctx_val, LAYOUT_SLOTS_DATA_OFF);
    let slots_chain = cg.fb.ins().iadd(slots_ptr_off, slots_data_off);
    let slots_chain = cg.fb.ins().iadd(props_off, slots_chain);
    let slots_addr = cg.fb.ins().iadd(obj_addr, slots_chain);
    let slots_ptr = cg.fb.ins().load(types::I64, MemFlags::new(), slots_addr, 0);
    let eight = cg.fb.ins().iconst(types::I64, 8);
    let byte_off = cg.fb.ins().imul(slot64, eight);
    let slot_addr = cg.fb.ins().iadd(slots_ptr, byte_off);
    let val = cg.fb.ins().load(types::I64, MemFlags::new(), slot_addr, 0);
    cg.fb.def_var(res, val);
    cg.jump_to(join_b);

    // slow：helper 全语义 + 回写缓存
    cg.goto(slow_b);
    let r = emit_helper(cg, fref_get, ctx_val, &[obj_box, key_idx, cell_addr]);
    cg.fb.def_var(res, r);
    cg.jump_to(join_b);

    cg.goto(join_b);
    cg.fb.use_var(res)
}

/// 属性写入 PIC：守卫链命中则槽位直写（`val` 为盒），否则 helper 回退。
/// 返回被写值（对齐 `SET_PROP` 压回语义）。
fn emit_set_prop(
    cg: &mut Cg,
    ctx_val: Value,
    obj_box: Value,
    key_idx: Value,
    val: Value,
    cell_addr: Value,
    fref_set: ir::FuncRef,
) -> Value {
    let slow_b = cg.fb.create_block();
    let join_b = cg.fb.create_block();
    let res = cg.new_var();

    let is_obj = cg.is_object(obj_box);
    let obj_ok_b = cg.fb.create_block();
    cg.fb.ins().brif(is_obj, obj_ok_b, &[], slow_b, &[]);
    cg.goto(obj_ok_b);

    let refv = cg.unbox_object_i64(obj_box);
    let heap = cg
        .fb
        .ins()
        .load(types::I64, MemFlags::new(), ctx_val, CTX_HEAP_PTR_OFF);
    let stride = cg
        .fb
        .ins()
        .load(types::I64, immutable_flags(), ctx_val, CTX_HEAP_STRIDE_OFF);
    let byte = cg.fb.ins().imul(refv, stride);
    let obj_addr = cg.fb.ins().iadd(heap, byte);

    let props_off = cg.layout_field(ctx_val, LAYOUT_PROPS_OFF);
    let layout_base = cg.fb.ins().iadd_imm(ctx_val, CTX_LAYOUT_OFF as i64);
    let disc_shape = cg.fb.ins().load(
        types::I32,
        immutable_flags(),
        layout_base,
        LAYOUT_DISC_SHAPE,
    );
    let deleted_gen_off = cg.layout_field(ctx_val, LAYOUT_DELETED_GEN_OFF);
    let has_acc_off = cg.layout_field(ctx_val, LAYOUT_HAS_ACCESSORS_OFF);

    let props_addr = cg.fb.ins().iadd(obj_addr, props_off);
    let props = cg.fb.ins().load(types::I32, MemFlags::new(), props_addr, 0);
    let dg_addr = cg.fb.ins().iadd(obj_addr, deleted_gen_off);
    let dg = cg.fb.ins().load(types::I32, MemFlags::new(), dg_addr, 0);
    let ha_addr = cg.fb.ins().iadd(obj_addr, has_acc_off);
    let ha = cg.fb.ins().load(types::I32, MemFlags::new(), ha_addr, 0);

    let is_shape = cg.fb.ins().icmp(IntCC::Equal, props, disc_shape);
    let no_del = cg.fb.ins().icmp_imm(IntCC::Equal, dg, 0);
    let no_acc = cg.fb.ins().icmp_imm(IntCC::Equal, ha, 0);
    let fast1 = cg.fb.ins().band(is_shape, no_del);
    let fast1 = cg.fb.ins().band(fast1, no_acc);

    // shape_id/slots 偏移相对 props 变体基址 → 须叠加 props_off
    let shape_id_off = cg.layout_field(ctx_val, LAYOUT_SHAPE_ID_OFF);
    let shape_chain = cg.fb.ins().iadd(props_off, shape_id_off);
    let shape_addr = cg.fb.ins().iadd(obj_addr, shape_chain);
    let shape = cg.fb.ins().load(types::I32, MemFlags::new(), shape_addr, 0);
    let state = cg
        .fb
        .ins()
        .load(types::I32, MemFlags::new(), cell_addr, PIC_STATE);
    let cached = cg
        .fb
        .ins()
        .load(types::I32, MemFlags::new(), cell_addr, PIC_SHAPE_ID);
    let state_ok = cg
        .fb
        .ins()
        .icmp_imm(IntCC::Equal, state, PicCell::FAST as i64);
    let shape_ok = cg.fb.ins().icmp(IntCC::Equal, shape, cached);
    let fast2 = cg.fb.ins().band(state_ok, shape_ok);
    let fast = cg.fb.ins().band(fast1, fast2);

    let fast_b = cg.fb.create_block();
    cg.fb.ins().brif(fast, fast_b, &[], slow_b, &[]);
    cg.goto(fast_b);

    // 槽位直写（盒）
    let slot = cg
        .fb
        .ins()
        .load(types::I32, MemFlags::new(), cell_addr, PIC_SLOT);
    let slot64 = cg.fb.ins().uextend(types::I64, slot);
    let slots_ptr_off = cg.layout_field(ctx_val, LAYOUT_SLOTS_PTR_OFF);
    let slots_data_off = cg.layout_field(ctx_val, LAYOUT_SLOTS_DATA_OFF);
    let slots_chain = cg.fb.ins().iadd(slots_ptr_off, slots_data_off);
    let slots_chain = cg.fb.ins().iadd(props_off, slots_chain);
    let slots_addr = cg.fb.ins().iadd(obj_addr, slots_chain);
    let slots_ptr = cg.fb.ins().load(types::I64, MemFlags::new(), slots_addr, 0);
    let eight = cg.fb.ins().iconst(types::I64, 8);
    let byte_off = cg.fb.ins().imul(slot64, eight);
    let slot_addr = cg.fb.ins().iadd(slots_ptr, byte_off);
    cg.fb.ins().store(MemFlags::new(), val, slot_addr, 0);
    cg.fb.def_var(res, val);
    cg.jump_to(join_b);

    cg.goto(slow_b);
    let r = emit_helper(cg, fref_set, ctx_val, &[obj_box, key_idx, val, cell_addr]);
    cg.fb.def_var(res, r);
    cg.jump_to(join_b);

    cg.goto(join_b);
    cg.fb.use_var(res)
}

/// 分配一个 PIC 站点内联缓存单元（可写数据段，零初始化）。
fn alloc_pic_cell(
    module: &mut JITModule,
    cg: &mut Cg,
    ptr_type: ir::Type,
    name: &str,
    bytes: usize,
) -> Result<Value, JitError> {
    let data_id = module
        .declare_data(name, Linkage::Local, true, false)
        .map_err(|e| JitError::Codegen(format!("declare data {name}: {e}")))?;
    let mut desc = DataDescription::new();
    desc.define(vec![0u8; bytes].into_boxed_slice());
    module
        .define_data(data_id, &desc)
        .map_err(|e| JitError::Codegen(format!("define data {name}: {e}")))?;
    let gv = module.declare_data_in_func(data_id, cg.fb.func);
    Ok(cg.fb.ins().global_value(ptr_type, gv))
}

/// 一个 `CALL` 站点的代码生成输入（避免长参数列表）。
struct CallSite {
    /// 被调盒
    callee: Value,
    /// 实参数组基址（本函数的栈槽）
    args_base: Value,
    /// 实参个数（I32 值，helper 用）
    argc_val: Value,
    /// 实参个数（编译期常量，直调用）
    argc: usize,
    /// 本站点调用内联缓存地址
    cell_addr: Value,
    /// `CALL` helper 的 FuncRef
    fref_call: ir::FuncRef,
    /// 直调的间接调用签名（与本函数签名同形）
    sig_ref: ir::SigRef,
    /// 目标机器指针类型
    ptr_type: ir::Type,
}

/// `CALL`：JIT→JIT 原生直调快速路径 + helper 回退。
///
/// 快速路径守卫（全部命中才直调）：站点缓存状态为 `FAST`、缓存的被调盒与
/// 本次被调**逐位相同**（闭包对象身份）、缓存代数 == `ctx.jit_gen`。命中后
/// 把 ctx 的常量池换成被调的（被调机器码按自己的常量池取属性名/全局名），
/// `call_indirect` 到被调入口，返回后复位常量池——省掉 helper 的盒↔`Value`
/// 转换、`Rc` 克隆与解释器帧字段换入换出。
///
/// 被调签名与本函数一致（`(ctx, args_ptr, len) -> u64`），实参直接复用调用方
/// 已写好的栈槽，无需重排。
fn emit_call(cg: &mut Cg, ctx_val: Value, site: &CallSite) -> Value {
    let CallSite {
        callee,
        args_base,
        argc_val,
        argc,
        cell_addr,
        fref_call,
        sig_ref,
        ptr_type,
    } = *site;
    let slow_b = cg.fb.create_block();
    let fast_b = cg.fb.create_block();
    let join_b = cg.fb.create_block();
    let res = cg.new_var();

    let state = cg
        .fb
        .ins()
        .load(types::I32, MemFlags::new(), cell_addr, CALL_STATE);
    let cached_callee = cg
        .fb
        .ins()
        .load(types::I64, MemFlags::new(), cell_addr, CALL_CALLEE);
    let cached_gen = cg
        .fb
        .ins()
        .load(types::I32, MemFlags::new(), cell_addr, CALL_GEN);
    let cur_gen = cg
        .fb
        .ins()
        .load(types::I32, MemFlags::new(), ctx_val, CTX_JIT_GEN_OFF);
    let state_ok = cg
        .fb
        .ins()
        .icmp_imm(IntCC::Equal, state, CallCell::FAST as i64);
    let callee_ok = cg.fb.ins().icmp(IntCC::Equal, cached_callee, callee);
    let gen_ok = cg.fb.ins().icmp(IntCC::Equal, cached_gen, cur_gen);
    let ok = cg.fb.ins().band(state_ok, callee_ok);
    let ok = cg.fb.ins().band(ok, gen_ok);
    cg.fb.ins().brif(ok, fast_b, &[], slow_b, &[]);

    // 快速路径：换常量池 → 原生调用 → 复位常量池
    cg.goto(fast_b);
    let entry = cg
        .fb
        .ins()
        .load(ptr_type, MemFlags::new(), cell_addr, CALL_ENTRY);
    let saved_cp = cg
        .fb
        .ins()
        .load(ptr_type, MemFlags::new(), ctx_val, CTX_CONSTS_PTR_OFF);
    let saved_cl = cg
        .fb
        .ins()
        .load(types::I64, MemFlags::new(), ctx_val, CTX_CONSTS_LEN_OFF);
    let new_cp = cg
        .fb
        .ins()
        .load(ptr_type, MemFlags::new(), cell_addr, CALL_CONSTS_PTR);
    let new_cl = cg
        .fb
        .ins()
        .load(types::I64, MemFlags::new(), cell_addr, CALL_CONSTS_LEN);
    cg.fb
        .ins()
        .store(MemFlags::new(), new_cp, ctx_val, CTX_CONSTS_PTR_OFF);
    cg.fb
        .ins()
        .store(MemFlags::new(), new_cl, ctx_val, CTX_CONSTS_LEN_OFF);
    let n = cg.fb.ins().iconst(types::I64, argc as i64);
    let inst = cg
        .fb
        .ins()
        .call_indirect(sig_ref, entry, &[ctx_val, args_base, n]);
    let r = cg.fb.inst_results(inst)[0];
    cg.fb
        .ins()
        .store(MemFlags::new(), saved_cp, ctx_val, CTX_CONSTS_PTR_OFF);
    cg.fb
        .ins()
        .store(MemFlags::new(), saved_cl, ctx_val, CTX_CONSTS_LEN_OFF);
    cg.fb.def_var(res, r);
    cg.jump_to(join_b);

    // 回退：helper 全语义（并回写本站点缓存，下轮可直调）
    cg.goto(slow_b);
    let h = emit_helper(
        cg,
        fref_call,
        ctx_val,
        &[callee, args_base, argc_val, cell_addr],
    );
    cg.fb.def_var(res, h);
    cg.jump_to(join_b);

    cg.goto(join_b);
    cg.fb.use_var(res)
}

/// 编译函数为机器码。
///
/// # Errors
/// 子集外操作码或 Cranelift 失败时返回 [`JitError`]。
pub fn jit_compile(func: &FuncTemplate, vtable: &JitVtable) -> Result<JittedFn, JitError> {
    let folded = peephole::const_fold(&func.code, &func.constants);
    let code = &folded.code;
    let constants = &folded.constants;

    let isa_builder =
        cranelift_native::builder().map_err(|e| JitError::Codegen(format!("native isa: {e}")))?;
    let mut flag_builder = settings::builder();
    flag_builder
        .set("use_colocated_libcalls", "false")
        .map_err(|e| JitError::Codegen(format!("flag: {e}")))?;
    flag_builder
        .set("is_pic", "false")
        .map_err(|e| JitError::Codegen(format!("flag: {e}")))?;
    // 优化级别 speed：开启 Cranelift 的 GVN/LICM/常量传播等中端优化。
    // 守卫链里大量重复的 ctx 字段加载（heap_ptr / layout 偏移）与地址算术
    // 靠这层消掉，编译时间的增量对本项目量级的函数可忽略。
    flag_builder
        .set("opt_level", "speed")
        .map_err(|e| JitError::Codegen(format!("flag: {e}")))?;
    let flags = settings::Flags::new(flag_builder);
    let isa = isa_builder
        .finish(flags)
        .map_err(|e| JitError::Codegen(format!("isa finish: {e}")))?;
    let call_conv = isa.default_call_conv();
    // helper 经模块 Import 解析（符号 → VM 的 extern "C" 函数地址）
    let v = *vtable;
    let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
    builder.symbol_lookup_fn(Box::new(move |name: &str| -> Option<*const u8> {
        match name {
            HELPER_GET_PROPERTY => Some(v.get_property as *const u8),
            HELPER_SET_PROPERTY => Some(v.set_property as *const u8),
            HELPER_ALLOC_ORDINARY => Some(v.alloc_ordinary as *const u8),
            HELPER_ADD => Some(v.add as *const u8),
            HELPER_EQ => Some(v.eq as *const u8),
            HELPER_STRICT_EQ => Some(v.strict_eq as *const u8),
            HELPER_TO_NUMBER => Some(v.to_number as *const u8),
            HELPER_TO_BOOLEAN => Some(v.to_boolean as *const u8),
            HELPER_CALL => Some(v.call as *const u8),
            HELPER_CALL_METHOD => Some(v.call_method as *const u8),
            HELPER_LOAD_GLOBAL => Some(v.load_global as *const u8),
            HELPER_LOAD_UPVALUE => Some(v.load_upvalue as *const u8),
            _ => None,
        }
    }));
    let mut module = JITModule::new(builder);
    let mut ctx = module.make_context();
    let ptr_type = module.target_config().pointer_type();
    // 签名：(ctx, args_ptr, len) -> u64；主函数调用约定取 native 默认
    ctx.func.signature.call_conv = call_conv;
    ctx.func.signature.params.push(AbiParam::new(ptr_type));
    ctx.func.signature.params.push(AbiParam::new(ptr_type));
    ctx.func.signature.params.push(AbiParam::new(types::I64));
    ctx.func.signature.returns.push(AbiParam::new(types::I64));
    // JIT→JIT 直调用的间接调用签名（与主函数签名同形）
    let jit_call_sig = ctx.func.signature.clone();

    // helper Import 声明（签名含 ctx 首参，与 Rust extern "C" 包装一致）
    let mk_sig = |params: &[ir::Type]| -> ir::Signature {
        let mut s = ir::Signature::new(call_conv);
        for &p in params {
            s.params.push(AbiParam::new(p));
        }
        s.returns.push(AbiParam::new(types::I64));
        s
    };
    let sig_ivu = mk_sig(&[ptr_type, types::I64, types::I32, ptr_type]);
    let sig_ivuv = mk_sig(&[ptr_type, types::I64, types::I32, types::I64, ptr_type]);
    let sig_ivv = mk_sig(&[ptr_type, types::I64, types::I64]);
    let sig_v = mk_sig(&[ptr_type]);
    let sig_i = mk_sig(&[ptr_type, types::I64]);
    fn decl(
        module: &mut JITModule,
        name: &str,
        sig: &ir::Signature,
    ) -> Result<cranelift_module::FuncId, JitError> {
        module
            .declare_function(name, Linkage::Import, sig)
            .map_err(|e| JitError::Codegen(format!("declare {name}: {e}")))
    }
    let id_get_prop = decl(&mut module, HELPER_GET_PROPERTY, &sig_ivu)?;
    let id_set_prop = decl(&mut module, HELPER_SET_PROPERTY, &sig_ivuv)?;
    let id_alloc = decl(&mut module, HELPER_ALLOC_ORDINARY, &sig_v)?;
    let id_add = decl(&mut module, HELPER_ADD, &sig_ivv)?;
    let id_eq = decl(&mut module, HELPER_EQ, &sig_ivv)?;
    let id_strict_eq = decl(&mut module, HELPER_STRICT_EQ, &sig_ivv)?;
    let id_tonum = decl(&mut module, HELPER_TO_NUMBER, &sig_i)?;
    let id_tobool = decl(&mut module, HELPER_TO_BOOLEAN, &sig_i)?;
    // (ctx, callee, args_ptr, argc, cell) -> u64
    let sig_call = mk_sig(&[ptr_type, types::I64, ptr_type, types::I32, ptr_type]);
    // (ctx, idx, cell) -> u64
    let sig_global = mk_sig(&[ptr_type, types::I32, ptr_type]);
    let sig_idx = mk_sig(&[ptr_type, types::I32]);
    let id_call = decl(&mut module, HELPER_CALL, &sig_call)?;
    // (ctx, receiver, name_idx, args_ptr, argc) -> u64
    let sig_callm = mk_sig(&[ptr_type, types::I64, types::I32, ptr_type, types::I32]);
    let id_call_method = decl(&mut module, HELPER_CALL_METHOD, &sig_callm)?;
    let id_load_global = decl(&mut module, HELPER_LOAD_GLOBAL, &sig_global)?;
    let id_load_upvalue = decl(&mut module, HELPER_LOAD_UPVALUE, &sig_idx)?;

    let mut fb_ctx = FunctionBuilderContext::new();
    let fb = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);
    let mut cg = Cg {
        fb,
        current: Block::new(0),
        var_next: func.num_locals.max(1) + 1,
        bool_boxes: std::collections::HashSet::new(),
    };
    // helper FuncRef（call 指令目标；任何时点 declare 均合法）
    let fref_get_prop = module.declare_func_in_func(id_get_prop, cg.fb.func);
    let fref_set_prop = module.declare_func_in_func(id_set_prop, cg.fb.func);
    let fref_alloc = module.declare_func_in_func(id_alloc, cg.fb.func);
    let fref_add = module.declare_func_in_func(id_add, cg.fb.func);
    let fref_eq = module.declare_func_in_func(id_eq, cg.fb.func);
    let fref_strict_eq = module.declare_func_in_func(id_strict_eq, cg.fb.func);
    let fref_tonum = module.declare_func_in_func(id_tonum, cg.fb.func);
    let fref_tobool = module.declare_func_in_func(id_tobool, cg.fb.func);
    let fref_call = module.declare_func_in_func(id_call, cg.fb.func);
    let fref_call_method = module.declare_func_in_func(id_call_method, cg.fb.func);
    let fref_load_global = module.declare_func_in_func(id_load_global, cg.fb.func);
    let fref_load_upvalue = module.declare_func_in_func(id_load_upvalue, cg.fb.func);
    // JIT→JIT 直调的间接调用签名引用（被调签名与本函数同形）
    let jit_sig_ref = cg.fb.import_signature(jit_call_sig);

    // 块集合：0、全部跳转目标/落点、末尾出口
    let mut starts: BTreeSet<usize> = BTreeSet::new();
    starts.insert(0);
    for (pc, instr) in code.iter().enumerate() {
        if matches!(
            instr.op,
            Op::Jmp | Op::JmpTruePop | Op::JmpFalsePop | Op::JmpTrueKeep | Op::JmpFalseKeep
        ) {
            starts.insert(jump_target(pc, instr.operand));
            starts.insert(pc + 1);
        }
    }
    starts.insert(code.len());
    let mut blocks: BTreeMap<usize, Block> = BTreeMap::new();
    let entry = *blocks.entry(0).or_insert_with(|| cg.fb.create_block());
    for &s in starts.iter().skip(1) {
        blocks.entry(s).or_insert_with(|| cg.fb.create_block());
    }

    cg.fb.append_block_params_for_function_params(entry);
    cg.goto(entry);
    // entry 立即封口（J1 同款）：线性后续 use_var 及时解析；回边指向 entry
    // 不在子集内（jitdiff 生成器常量初始化先导，跳转目标 > 0）
    cg.fb.seal_block(entry);

    // CALL 实参暂存槽（一次分配、逐调用复用；容量取 max_stack 上限）
    let call_slots = func.max_stack.clamp(8, 256);
    let call_args_slot = cg.fb.create_sized_stack_slot(ir::StackSlotData::new(
        ir::StackSlotKind::ExplicitSlot,
        call_slots * 8,
        3,
    ));

    // 局部槽位 → Cranelift Variable（I64 盒；槽 0 = this = undefined 占位）
    let num_locals = func.num_locals.max(1) as usize;
    let mut vars: Vec<Variable> = Vec::with_capacity(num_locals);
    for i in 0..num_locals {
        let v = Variable::from_u32(i as u32);
        cg.fb.declare_var(v, types::I64);
        let init = { cg.fb.ins().iconst(types::I64, UNDEFINED as i64) };
        cg.fb.def_var(v, init);
        vars.push(v);
    }
    // 实参装载：args[i] → vars[1+i]
    let params = cg.fb.block_params(entry);
    let ctx_val = params[0];
    let args_ptr = params[1];
    let param_count = (func.num_params as usize).min(num_locals.saturating_sub(1));
    for (i, &var) in vars.iter().enumerate().skip(1).take(param_count) {
        let idx = { cg.fb.ins().iconst(types::I64, (i - 1) as i64) };
        let eight = { cg.fb.ins().iconst(types::I64, 8) };
        let off = { cg.fb.ins().imul(idx, eight) };
        let addr = { cg.fb.ins().iadd(args_ptr, off) };
        let loaded = { cg.fb.ins().load(types::I64, MemFlags::new(), addr, 0) };
        cg.fb.def_var(var, loaded);
    }

    let mut value_stack: Vec<Value> = Vec::new();
    let mut terminated = false;
    // 是否读上值（读上值的函数不可作为 JIT→JIT 直调被调；见 `JittedFn`）
    let mut uses_upvalues = false;
    // 入口：活跃 JIT 帧 +1（GC 在帧内不回收——机器码局部无栈映射）
    cg.bump_frames(ctx_val, ptr_type, 1);

    let mut pc = 0usize;
    while pc < code.len() {
        // 块边界：切到本指令所属块（若与当前块不同）
        if let Some(&b) = blocks.get(&pc) {
            if b != cg.current {
                if !terminated {
                    cg.jump_to(b);
                }
                cg.goto(b);
                terminated = false;
                value_stack.clear();
            }
        } else if terminated {
            // 不可达且非块起点：跳过
            pc += 1;
            continue;
        }
        let instr = &code[pc];
        match instr.op {
            Op::PushConst => {
                let v = match constants.get(instr.operand as usize) {
                    Some(Constant::Number(n)) => box_number(*n),
                    Some(Constant::Bool(b)) => {
                        if *b {
                            TRUE
                        } else {
                            FALSE
                        }
                    }
                    Some(Constant::Null) => NULL,
                    _ => return Err(unsupported(func, pc)),
                };
                value_stack.push(cg.fb.ins().iconst(types::I64, v as i64));
            }
            Op::PushInt => {
                let v = box_number(instr.operand as f64);
                value_stack.push(cg.fb.ins().iconst(types::I64, v as i64));
            }
            Op::PushNegInt => {
                let v = box_number(-(instr.operand as f64));
                value_stack.push(cg.fb.ins().iconst(types::I64, v as i64));
            }
            Op::PushTrue => {
                let v = cg.fb.ins().iconst(types::I64, TRUE as i64);
                cg.bool_boxes.insert(v);
                value_stack.push(v);
            }
            Op::PushFalse => {
                let v = cg.fb.ins().iconst(types::I64, FALSE as i64);
                cg.bool_boxes.insert(v);
                value_stack.push(v);
            }
            Op::PushUndefined => {
                value_stack.push(cg.fb.ins().iconst(types::I64, UNDEFINED as i64));
            }
            Op::LoadLocal => {
                let slot = instr.operand as usize;
                if slot == 0 || slot >= vars.len() {
                    // this 槽：ABI 不传 this（仅 undefined 占位），读取会失真 →
                    // 编译期拒绝（调用方资格判定不编译读 this 的函数）
                    return Err(unsupported(func, pc));
                }
                value_stack.push(cg.fb.use_var(vars[slot]));
            }
            Op::StoreLocal => {
                let slot = instr.operand as usize;
                let Some(v) = value_stack.pop() else {
                    return Err(JitError::Codegen("栈下溢".into()));
                };
                if slot == 0 || slot >= vars.len() {
                    return Err(JitError::Codegen(format!("槽位越界 {slot}")));
                }
                cg.fb.def_var(vars[slot], v);
            }
            Op::Dup => {
                let top = *value_stack
                    .last()
                    .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                value_stack.push(top);
            }
            Op::Swap => {
                let b = value_stack
                    .pop()
                    .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                let a = value_stack
                    .pop()
                    .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                value_stack.push(b);
                value_stack.push(a);
            }
            Op::Add | Op::Sub | Op::Mul | Op::Div => {
                let b = value_stack
                    .pop()
                    .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                let a = value_stack
                    .pop()
                    .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                if instr.op == Op::Add {
                    // ADD 需完整 add_values 语义（字符串拼接）→ 数值快速 + helper 回退
                    value_stack.push(emit_add(&mut cg, ctx_val, a, b, fref_add));
                } else {
                    value_stack.push(emit_arith(&mut cg, ctx_val, a, b, instr.op, fref_tonum));
                }
            }
            Op::LoadGlobal => {
                let idx = key_value(&mut cg, instr.operand);
                let cell = alloc_pic_cell(
                    &mut module,
                    &mut cg,
                    ptr_type,
                    &format!("global_{pc}"),
                    std::mem::size_of::<GlobalCell>(),
                )?;
                let r = emit_load_global(&mut cg, ctx_val, idx, cell, fref_load_global);
                value_stack.push(r);
            }
            Op::LoadUpvalue => {
                uses_upvalues = true;
                let idx = key_value(&mut cg, instr.operand);
                let r = emit_helper(&mut cg, fref_load_upvalue, ctx_val, &[idx]);
                value_stack.push(r);
            }
            Op::Call => {
                // 实参写入本函数的调用参数暂存槽（栈槽，避免每次分配）
                let argc = instr.operand as usize;
                if value_stack.len() < argc + 1 {
                    return Err(JitError::Codegen("栈下溢".into()));
                }
                let mut vals = Vec::with_capacity(argc);
                for _ in 0..argc {
                    vals.push(
                        value_stack
                            .pop()
                            .ok_or_else(|| JitError::Codegen("栈下溢".into()))?,
                    );
                }
                vals.reverse();
                let callee = value_stack
                    .pop()
                    .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                let args_base = cg.fb.ins().stack_addr(ptr_type, call_args_slot, 0);
                for (i, v) in vals.iter().enumerate() {
                    cg.fb
                        .ins()
                        .store(MemFlags::new(), *v, args_base, (i * 8) as i32);
                }
                let argc_val = cg.fb.ins().iconst(types::I32, argc as i64);
                // 调用内联缓存：命中即 call_indirect 直调被调机器码（不经 helper）
                let cell = alloc_pic_cell(
                    &mut module,
                    &mut cg,
                    ptr_type,
                    &format!("callic_{pc}"),
                    std::mem::size_of::<CallCell>(),
                )?;
                let r = emit_call(
                    &mut cg,
                    ctx_val,
                    &CallSite {
                        callee,
                        args_base,
                        argc_val,
                        argc,
                        cell_addr: cell,
                        fref_call,
                        sig_ref: jit_sig_ref,
                        ptr_type,
                    },
                );
                value_stack.push(r);
            }
            Op::Mod | Op::Pow => {
                // MOD/POW 需 f64 取余/幂，Cranelift 无直接指令 → 子集外回退解释器
                return Err(unsupported(func, pc));
            }
            Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                let b = value_stack
                    .pop()
                    .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                let a = value_stack
                    .pop()
                    .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                value_stack.push(emit_cmp(&mut cg, ctx_val, a, b, instr.op, fref_tonum));
            }
            Op::Eq | Op::Ne | Op::StrictEq | Op::StrictNe => {
                let b = value_stack
                    .pop()
                    .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                let a = value_stack
                    .pop()
                    .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                value_stack.push(emit_eq(
                    &mut cg,
                    ctx_val,
                    a,
                    b,
                    instr.op,
                    fref_eq,
                    fref_strict_eq,
                ));
            }
            Op::Jmp => {
                let target = jump_target(pc, instr.operand);
                let b = *blocks
                    .get(&target)
                    .ok_or_else(|| JitError::Codegen(format!("跳转目标越界 {target}")))?;
                cg.jump_to(b);
                terminated = true;
            }
            Op::JmpTruePop | Op::JmpFalsePop | Op::JmpTrueKeep | Op::JmpFalseKeep => {
                let cond = if matches!(instr.op, Op::JmpTruePop | Op::JmpFalsePop) {
                    value_stack
                        .pop()
                        .ok_or_else(|| JitError::Codegen("栈下溢".into()))?
                } else {
                    *value_stack
                        .last()
                        .ok_or_else(|| JitError::Codegen("栈下溢".into()))?
                };
                let target = jump_target(pc, instr.operand);
                let tb = *blocks
                    .get(&target)
                    .ok_or_else(|| JitError::Codegen(format!("跳转目标越界 {target}")))?;
                let fb_b = *blocks
                    .get(&(pc + 1))
                    .ok_or_else(|| JitError::Codegen("跳转落点缺块".into()))?;
                let truthy = emit_truthy(&mut cg, ctx_val, cond, fref_tobool);
                let (taken, fall) = match instr.op {
                    Op::JmpTruePop | Op::JmpTrueKeep => (tb, fb_b),
                    _ => (fb_b, tb),
                };
                cg.fb.ins().brif(truthy, taken, &[], fall, &[]);
                terminated = true;
            }
            Op::Inc | Op::Dec => {
                let a = value_stack
                    .pop()
                    .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                let n = emit_to_number(&mut cg, ctx_val, a, fref_tonum);
                let nf = cg.unbox(n);
                let one = cg.fb.ins().f64const(1.0);
                let r = if instr.op == Op::Inc {
                    cg.fb.ins().fadd(nf, one)
                } else {
                    cg.fb.ins().fsub(nf, one)
                };
                value_stack.push(cg.boxed(r));
            }
            Op::Neg => {
                let a = value_stack
                    .pop()
                    .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                let n = emit_to_number(&mut cg, ctx_val, a, fref_tonum);
                let nf = cg.unbox(n);
                let r = cg.fb.ins().fneg(nf);
                value_stack.push(cg.boxed(r));
            }
            Op::Not => {
                let a = value_stack
                    .pop()
                    .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                let t = emit_truthy(&mut cg, ctx_val, a, fref_tobool);
                let f = cg.fb.ins().iconst(types::I64, FALSE as i64);
                let tt = cg.fb.ins().iconst(types::I64, TRUE as i64);
                let r = cg.fb.ins().select(t, f, tt);
                cg.bool_boxes.insert(r);
                value_stack.push(r);
            }
            Op::GetProp | Op::GetPropLocal => {
                let key_idx = if instr.op == Op::GetProp {
                    instr.operand
                } else {
                    instr.operand & 0xFFFF
                };
                let obj = if instr.op == Op::GetPropLocal {
                    let slot = (instr.operand >> 16) as usize;
                    if slot == 0 || slot >= vars.len() {
                        return Err(JitError::Codegen(format!("槽位越界 {slot}")));
                    }
                    cg.fb.use_var(vars[slot])
                } else {
                    value_stack
                        .pop()
                        .ok_or_else(|| JitError::Codegen("栈下溢".into()))?
                };
                // PIC：内联缓存单元 + 守卫链直读，失配 helper 回退
                let cell = alloc_pic_cell(
                    &mut module,
                    &mut cg,
                    ptr_type,
                    &format!("pic_get_{pc}"),
                    std::mem::size_of::<PicCell>(),
                )?;
                let kv = key_value(&mut cg, key_idx);
                let r = emit_get_prop(&mut cg, ctx_val, obj, kv, cell, fref_get_prop);
                value_stack.push(r);
            }
            Op::SetProp | Op::SetPropObj | Op::SetPropTop => {
                let key_idx = instr.operand;
                let (val, obj) = match instr.op {
                    Op::SetProp => {
                        let v = value_stack
                            .pop()
                            .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                        let o = value_stack
                            .pop()
                            .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                        (v, o)
                    }
                    Op::SetPropObj => {
                        let v = value_stack
                            .pop()
                            .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                        let o = *value_stack
                            .last()
                            .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                        (v, o)
                    }
                    Op::SetPropTop => {
                        let o = value_stack
                            .pop()
                            .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                        let v = value_stack
                            .pop()
                            .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                        (v, o)
                    }
                    _ => unreachable!(),
                };
                // PIC：守卫链命中直写槽位，失配 helper 回退
                let cell = alloc_pic_cell(
                    &mut module,
                    &mut cg,
                    ptr_type,
                    &format!("pic_set_{pc}"),
                    std::mem::size_of::<PicCell>(),
                )?;
                let kv = key_value(&mut cg, key_idx);
                let r = emit_set_prop(&mut cg, ctx_val, obj, kv, val, cell, fref_set_prop);
                // SET_PROP 压回被写值；SET_PROP_OBJ / SET_PROP_TOP 不压
                if instr.op == Op::SetProp {
                    value_stack.push(r);
                }
            }
            Op::CallMethod => {
                // 操作数 = argc<<16 | name_idx；栈序 [..., receiver, arg1..argN]
                let argc = (instr.operand >> 16) as usize;
                let name_idx = (instr.operand & 0xFFFF) as usize;
                if value_stack.len() < argc + 1 {
                    return Err(JitError::Codegen("栈下溢".into()));
                }
                let mut vals = Vec::with_capacity(argc);
                for _ in 0..argc {
                    vals.push(
                        value_stack
                            .pop()
                            .ok_or_else(|| JitError::Codegen("栈下溢".into()))?,
                    );
                }
                vals.reverse();
                let receiver = value_stack
                    .pop()
                    .ok_or_else(|| JitError::Codegen("栈下溢".into()))?;
                let args_base = cg.fb.ins().stack_addr(ptr_type, call_args_slot, 0);
                for (i, v) in vals.iter().enumerate() {
                    cg.fb
                        .ins()
                        .store(MemFlags::new(), *v, args_base, (i * 8) as i32);
                }
                let argc_val = cg.fb.ins().iconst(types::I32, argc as i64);
                let name_idx_val = cg.fb.ins().iconst(types::I32, name_idx as i64);
                // 全语义经解释器统一分派链（call_method_dispatch 单源）
                let inst = cg.fb.ins().call(
                    fref_call_method,
                    &[ctx_val, receiver, name_idx_val, args_base, argc_val],
                );
                value_stack.push(cg.fb.inst_results(inst)[0]);
            }
            Op::NewObject => {
                if instr.operand != 0 {
                    // 带属性对的新对象：子集外（需 to_property_key 语义），回退解释器
                    return Err(unsupported(func, pc));
                }
                let r = emit_helper(&mut cg, fref_alloc, ctx_val, &[]);
                value_stack.push(r);
            }
            Op::Return => {
                let Some(v) = value_stack.pop() else {
                    return Err(JitError::Codegen("栈下溢".into()));
                };
                cg.bump_frames(ctx_val, ptr_type, -1);
                cg.fb.ins().return_(&[v]);
                terminated = true;
            }
            Op::Pop => {
                value_stack.pop();
            }
            Op::Nop => {}
            _ => return Err(unsupported(func, pc)),
        }
        pc += 1;
        // 已终结且下一 pc 非块起点：切换到出口占位（后续不可达跳过）
        if terminated {
            if let Some(&b) = blocks.get(&pc) {
                cg.goto(b);
                terminated = false;
                value_stack.clear();
            }
        }
    }
    // 出口块：一切未终结路径跳入；统一返回 undefined（正常路径已在 Return 终结）
    let exit = *blocks.get(&code.len()).expect("出口块已创建");
    if cg.current != exit && !terminated {
        cg.jump_to(exit);
    }
    cg.goto(exit);
    let undef = cg.fb.ins().iconst(types::I64, UNDEFINED as i64);
    cg.bump_frames(ctx_val, ptr_type, -1);
    cg.fb.ins().return_(&[undef]);
    // 全部块在此统一封口（回边已发射，frontend 解析延迟变量）
    cg.fb.seal_all_blocks();

    let func_id = module
        .declare_function(&func.name, Linkage::Local, &ctx.func.signature)
        .map_err(|e| JitError::Codegen(format!("declare: {e}")))?;
    module
        .define_function(func_id, &mut ctx)
        .map_err(|e| JitError::Codegen(format!("define: {e:?}")))?;
    module.clear_context(&mut ctx);
    module
        .finalize_definitions()
        .map_err(|e| JitError::Codegen(format!("finalize: {e}")))?;
    let ptr = module.get_finalized_function(func_id);
    Ok(JittedFn {
        ptr,
        module,
        folded_len: code.len(),
        uses_upvalues,
    })
}

fn unsupported(func: &FuncTemplate, pc: usize) -> JitError {
    let op = func.code.get(pc).map(|i| i.op).unwrap_or(Op::Nop);
    JitError::UnsupportedOpcode {
        func: format!("{} (orig pc {pc}, op {op:?})", func.name),
        pc,
    }
}

/// 跳转目标指令索引（对齐 VM `compute_jump_target`：相对下一指令的字节偏移）。
#[must_use]
pub fn jump_target(pc: usize, operand: u32) -> usize {
    let signed = if operand & 0x80_0000 != 0 {
        (operand | 0xFF00_0000) as i32
    } else {
        operand as i32
    };
    (((pc as i32 * 4) + 4 + signed) / 4) as usize
}
