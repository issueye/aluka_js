//! JIT 与 VM 的运行时桥：`JitCtx` 携带 VM 状态指针、常量池、堆布局与
//! helper 函数指针表，JIT 机器码经其回调解释器完成子集外语义。
//!
//! 布局纪律（AGENTS 约束 1 的 JIT 例外）：
//! - `JitCtx` / `JitVtable` / `JitLayout` 均为 `#[repr(C)]`，字段偏移对机器码稳定；
//! - 单线程同步调用：JIT 函数由解释器在同一线程内同步驱动，`ctx.vm` 指向的
//!   `&mut Vm` 在 JIT 执行期间无其它别名；
//! - 堆基址 `heap_ptr` 仅在某一个基本操作内有效：helper 可能分配触发 GC（`Vec`
//!   扩容），任何 helper 调用后 JIT 必须重新从 `ctx` 读 `heap_ptr`。

use crate::valbox;
use aluka_bytecode::Constant;

/// 内联缓存单元（PIC）：每个属性访问点位一个，JIT 直读、helper 失配时回写。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PicCell {
    /// 命中缓存时的对象隐藏类 id
    pub shape_id: u32,
    /// 命中缓存时的槽位下标
    pub slot: u32,
    /// 缓存状态：0=未初始化 / 1=shape 快速 / 2=不可快速
    pub state: u32,
}

impl PicCell {
    /// 未初始化
    pub const UNINIT: u32 = 0;
    /// shape 快速路径可用
    pub const FAST: u32 = 1;
    /// 对象不可走快速路径（字典模式/访问器/删除等）
    pub const NO_FAST: u32 = 2;
    /// 构造全零缓存单元。
    #[must_use]
    pub fn zeros() -> Self {
        Self {
            shape_id: 0,
            slot: 0,
            state: Self::UNINIT,
        }
    }
}

/// 方法调用（`CALL_METHOD`）：receiver+方法名+实参表 → 解释器统一分派链
/// （`call_method_dispatch`，与解释器 `Op::CallMethod` 单源同语义）。
pub type CallMethodFn = unsafe extern "C" fn(
    ctx: *mut JitCtx,
    receiver: u64,
    name_idx: u32,
    args_ptr: *const u64,
    argc: u32,
) -> u64;

/// 构造调用（`NEW`）：实参表 → 解释器 `do_construct`。
pub type ConstructFn =
    unsafe extern "C" fn(ctx: *mut JitCtx, callee: u64, args_ptr: *const u64, argc: u32) -> u64;

/// 数组实参调用族（`CALL_ARGS`/`NEW_ARGS`）：实参数组值 → 解释器展开。
pub type CallArgsFn = unsafe extern "C" fn(ctx: *mut JitCtx, callee: u64, args_array: u64) -> u64;

/// this 绑定调用族（`CALL_WITH_THIS`/`CALL_WITH_THIS_ARGS`）：
/// 显式 this + 实参（表指针或数组值，由 `argc` 区分）→ 解释器 `invoke_callable`。
pub type CallThisFn = unsafe extern "C" fn(
    ctx: *mut JitCtx,
    callee: u64,
    this_val: u64,
    args_ptr_or_array: u64,
    argc: u32,
) -> u64;

/// 全局读取内联缓存：仅缓存 `vm.globals` 中已经存在的键。
///
/// 内建动态全局（`Math`、`URL`、`JSON` 等）继续走 helper，因为解析本身可能
/// 分配对象；`globals_gen` 变化时缓存失效，避免公开 `Vm::globals` 被外部修改后
/// 读到旧值。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GlobalCell {
    /// 站点缓存的常量池键下标（诊断/防误用）
    pub key_idx: u32,
    /// 缓存状态：0=未初始化 / 1=可直读 / 2=不可缓存
    pub state: u32,
    /// 缓存的 globals 变更代数
    pub globals_gen: u32,
    /// 对齐填充，保持后续字段 8 字节对齐
    pub _pad: u32,
    /// 缓存的 NaN-box 值
    pub value: u64,
}

impl GlobalCell {
    /// 未初始化
    pub const UNINIT: u32 = 0;
    /// 可直读
    pub const FAST: u32 = 1;
    /// 不可缓存（动态内建或缺失）
    pub const NO_FAST: u32 = 2;
}

/// 调用内联缓存单元（JIT→JIT 原生直调）：每个 `CALL` 站点一个。
///
/// 命中时 JIT 直接 `call_indirect` 到被调机器码入口，**不经 helper**——省掉
/// 盒↔`Value` 转换、`Rc` 克隆、帧字段换入换出。守卫三件事：站点缓存的被调
/// 身份、JIT 代数（模块替换后旧入口可能已释放）、缓存状态。
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CallCell {
    /// 命中缓存时的被调盒（闭包对象身份；直调只对同一闭包对象有效）
    pub callee: u64,
    /// 被调机器码入口地址（0 = 不可直调）
    pub entry: usize,
    /// 被调常量池基址（直调前换入 `ctx.consts_ptr`，返回后复位）
    pub consts_ptr: usize,
    /// 被调常量池长度
    pub consts_len: usize,
    /// 登记时的 JIT 代数（≠ [`JitCtx::jit_gen`] 即失效，回 helper 重新解析）
    pub cell_gen: u32,
    /// 缓存状态：0=未初始化 / 1=可直调 / 2=不可直调
    pub state: u32,
    /// 被调上值表数据基址（机器可寻址；直调前换入 `ctx.upvals_ptr`）。
    ///
    /// **不被缓存守卫依赖**：emit_call 快路径每次从被调堆对象现读
    /// upvalues 表指针（经 `JitLayout::closure_uv_*_off`），不存在陈旧
    /// 缓存指针问题；此字段仅为省一次站点内 load 而设（暂留 0）。
    pub upvals_ptr: usize,
    /// 被调上值表长度（与 [`CallCell::upvals_ptr`] 成对）
    pub upvals_len: usize,
}

impl CallCell {
    /// 未初始化
    pub const UNINIT: u32 = 0;
    /// 可原生直调
    pub const FAST: u32 = 1;
    /// 不可直调（被调未编译/带上值/读上值/实参不足），走 helper
    pub const NO_FAST: u32 = 2;
    /// 构造全零缓存单元。
    #[must_use]
    pub fn zeros() -> Self {
        Self {
            callee: 0,
            entry: 0,
            consts_ptr: 0,
            consts_len: 0,
            cell_gen: 0,
            state: Self::UNINIT,
            upvals_ptr: 0,
            upvals_len: 0,
        }
    }
}

/// 属性读取：完整语义（访问器/原型链/数组/字符串接收者）；回写 cache。
pub type GetPropFn =
    unsafe extern "C" fn(ctx: *mut JitCtx, obj: u64, key_idx: u32, cell: *mut PicCell) -> u64;
/// 属性写入：完整语义；返回被写值（对齐 `SET_PROP` 压回）；回写 cache。
pub type SetPropFn = unsafe extern "C" fn(
    ctx: *mut JitCtx,
    obj: u64,
    key_idx: u32,
    val: u64,
    cell: *mut PicCell,
) -> u64;
/// 分配空普通对象（`NEW_OBJECT` 0 属性形态）。
pub type AllocOrdinaryFn = unsafe extern "C" fn(ctx: *mut JitCtx) -> u64;
/// 字符串拼接语义（`ADD`：全 `ToPrimitive` + 拼接，见 VM `add_values`）。
pub type AddFn = unsafe extern "C" fn(ctx: *mut JitCtx, a: u64, b: u64) -> u64;
/// 非严格相等（`EQ`，全语义）。
pub type EqFn = unsafe extern "C" fn(ctx: *mut JitCtx, a: u64, b: u64) -> u64;
/// 严格相等（`STRICT_EQ`）。
pub type StrictEqFn = unsafe extern "C" fn(ctx: *mut JitCtx, a: u64, b: u64) -> u64;
/// 强制转数值（`ToNumber`），返回数值盒。
pub type ToNumberFn = unsafe extern "C" fn(ctx: *mut JitCtx, a: u64) -> u64;
/// 强制转布尔（`ToBoolean`），返回 TRUE/FALSE 盒。
pub type ToBooleanFn = unsafe extern "C" fn(ctx: *mut JitCtx, a: u64) -> u64;
/// 调用（`CALL`）：`callee` 为可调用盒，实参经 `args_ptr`/`argc` 传入
/// （JIT 侧栈槽数组），语义完全由解释器 `invoke_callable` 决定；`cell` 为本
/// 站点的调用内联缓存，helper 判定被调可原生直调时回写（下轮 JIT 直连）。
pub type CallFn = unsafe extern "C" fn(
    ctx: *mut JitCtx,
    callee: u64,
    args_ptr: *const u64,
    argc: u32,
    cell: *mut CallCell,
) -> u64;
/// 全局读取（`LOAD_GLOBAL`）：按常量池下标取名字后走解释器全局解析；`cell` 为
/// 该位点的 Global IC，helper 对 `vm.globals` 直接命中时回写。
pub type LoadGlobalFn =
    unsafe extern "C" fn(ctx: *mut JitCtx, name_idx: u32, cell: *mut GlobalCell) -> u64;
/// 上值读取（`LOAD_UPVALUE`）：读当前帧上值表（由 `jit_run` 安装）。
pub type LoadUpvalueFn = unsafe extern "C" fn(ctx: *mut JitCtx, uv_idx: u32) -> u64;

/// `TYPEOF`：typeof 语义（解释器 `typeof_value` 单源），返回字符串对象盒。
pub type TypeofFn = unsafe extern "C" fn(ctx: *mut JitCtx, v: u64) -> u64;
/// `TYPEOF_GLOBAL`：全局 typeof（动态解析不缓存）。
pub type TypeofGlobalFn = unsafe extern "C" fn(ctx: *mut JitCtx, name_idx: u32) -> u64;
/// `GET_ELEM`：动态键属性读取。
pub type GetElemFn = unsafe extern "C" fn(ctx: *mut JitCtx, obj: u64, key: u64) -> u64;
/// `SET_ELEM`：动态键属性写入，返回被写值。
pub type SetElemFn = unsafe extern "C" fn(ctx: *mut JitCtx, obj: u64, key: u64, val: u64) -> u64;
/// `DEL_PROP`：属性删除。
pub type DelPropFn = unsafe extern "C" fn(ctx: *mut JitCtx, obj: u64, name_idx: u32) -> u64;
/// `GET_PROTO`：`[[Prototype]]` 读取。
pub type GetProtoFn = unsafe extern "C" fn(ctx: *mut JitCtx, obj: u64) -> u64;
/// `INSTANCEOF`。
pub type InstanceofFn = unsafe extern "C" fn(ctx: *mut JitCtx, l: u64, r: u64) -> u64;
/// `IN`。
pub type InFn = unsafe extern "C" fn(ctx: *mut JitCtx, key: u64, obj: u64) -> u64;
/// `NEW_ARRAY`/`BUILD_ARRAY`：n 个盒 → 数组对象。
pub type NewArrayFn = unsafe extern "C" fn(ctx: *mut JitCtx, vals_ptr: *const u64, n: u32) -> u64;
/// `ARRAY_PUSH`：追加元素（含写屏障）。
pub type ArrayPushFn = unsafe extern "C" fn(ctx: *mut JitCtx, arr: u64, val: u64) -> u64;

/// 位运算族（`BIT_AND/OR/XOR/SHL/SHR/USHR/NOT`）：ToNumber + i32 位语义。
/// `op`：0=And 1=Or 2=Xor 3=Shl 4=Shr 5=UShr 6=Not。
pub type BitOpFn = unsafe extern "C" fn(ctx: *mut JitCtx, a: u64, b: u64, op: u32) -> u64;
/// `STORE_GLOBAL`：CJS 注入名进模块作用域，其余进全局表。
pub type StoreGlobalFn = unsafe extern "C" fn(ctx: *mut JitCtx, name_idx: u32, val: u64) -> u64;
/// `DEL_ELEM`：动态键删除。
pub type DelElemFn = unsafe extern "C" fn(ctx: *mut JitCtx, obj: u64, key: u64) -> u64;
/// 访问器注册（`SET_GETTER_OBJ`/`SET_SETTER_OBJ` 及 Computed 变体共用）：
/// `key_is_box` 区分常量名 idx 与动态键盒。
pub type SetAccessorFn = unsafe extern "C" fn(
    ctx: *mut JitCtx,
    obj: u64,
    key: u64,
    fn_val: u64,
    is_setter: bool,
    key_is_box: bool,
) -> u64;
/// `STORE_UPVALUE`：写当前帧上值表单元格。
pub type StoreUpvalueFn = unsafe extern "C" fn(ctx: *mut JitCtx, uv_idx: u32, val: u64) -> u64;
/// `SPREAD_OBJECT`。
pub type SpreadObjectFn = unsafe extern "C" fn(ctx: *mut JitCtx, src: u64, dst: u64) -> u64;
/// `ENUM_KEYS`：for-in 键快照。
pub type EnumKeysFn = unsafe extern "C" fn(ctx: *mut JitCtx, src: u64) -> u64;
/// `ARRAY_SPREAD`：迭代物化 + 追加（错误降级空集，J2 约定）。
pub type ArraySpreadFn =
    unsafe extern "C" fn(ctx: *mut JitCtx, target_arr: u64, spread_val: u64) -> u64;

/// helper 函数指针表（由 aluka-vm 每次调用时填充）。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct JitVtable {
    /// 属性读取（全语义）
    pub get_property: GetPropFn,
    /// 属性写入（全语义）
    pub set_property: SetPropFn,
    /// 分配空普通对象
    pub alloc_ordinary: AllocOrdinaryFn,
    /// `ADD` 字符串拼接语义
    pub add: AddFn,
    /// `EQ` 非严格相等
    pub eq: EqFn,
    /// `STRICT_EQ`
    pub strict_eq: StrictEqFn,
    /// `ToNumber`
    pub to_number: ToNumberFn,
    /// `ToBoolean`
    pub to_boolean: ToBooleanFn,
    /// `CALL`（含闭包调用；语义由解释器 `invoke_callable` 决定）
    pub call: CallFn,
    /// `CALL_METHOD`（全语义经解释器统一分派链）
    pub call_method: CallMethodFn,
    /// `NEW`（语义由解释器 `do_construct` 决定）
    pub construct: ConstructFn,
    /// `CALL_ARGS`/`NEW_ARGS`（实参数组展开）
    pub call_args: CallArgsFn,
    /// `CALL_WITH_THIS`/`CALL_WITH_THIS_ARGS`（this 绑定，两变体共用）
    pub call_this: CallThisFn,
    /// `LOAD_GLOBAL`
    pub load_global: LoadGlobalFn,
    /// `LOAD_UPVALUE`
    pub load_upvalue: LoadUpvalueFn,
    /// `TYPEOF`
    pub typeof_: TypeofFn,
    /// `TYPEOF_GLOBAL`
    pub typeof_global: TypeofGlobalFn,
    /// `GET_ELEM`
    pub get_elem: GetElemFn,
    /// `SET_ELEM`
    pub set_elem: SetElemFn,
    /// `DEL_PROP`
    pub del_prop: DelPropFn,
    /// `GET_PROTO`
    pub get_proto: GetProtoFn,
    /// `INSTANCEOF`
    pub instanceof: InstanceofFn,
    /// `IN`
    pub in_: InFn,
    /// `NEW_ARRAY`/`BUILD_ARRAY`
    pub new_array: NewArrayFn,
    /// `ARRAY_PUSH`
    pub array_push: ArrayPushFn,
    /// 位运算族（op 选择）
    pub bitop: BitOpFn,
    /// `STORE_GLOBAL`
    pub store_global: StoreGlobalFn,
    /// `DEL_ELEM`
    pub del_elem: DelElemFn,
    /// 访问器注册（Getter/Setter 及 Computed 变体共用）
    pub set_accessor: SetAccessorFn,
    /// `STORE_UPVALUE`（写机器可寻址上值表单元格）
    pub store_upvalue: StoreUpvalueFn,
    /// `SPREAD_OBJECT`
    pub spread_object: SpreadObjectFn,
    /// `ENUM_KEYS`
    pub enum_keys: EnumKeysFn,
    /// `ARRAY_SPREAD`
    pub array_spread: ArraySpreadFn,
}

/// 对象布局偏移（PIC 快速路径用；由 aluka-vm 按实际布局填充）。
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct JitLayout {
    /// `HeapObject::Ordinary.props` 字段偏移（判别式起点）
    pub props_off: usize,
    /// `OrdinaryProps::Shape.shape_id` 在对象内的偏移
    pub shape_id_off: usize,
    /// `OrdinaryProps::Shape.slots`（Vec 结构体）在对象内的偏移
    pub slots_ptr_off: usize,
    /// 数据指针在 `Vec` 结构体内的偏移。
    ///
    /// **不要假设为 0**：`Vec` 是 `repr(Rust)`，字段顺序由编译器决定（本机
    /// 实测数据指针在 +8，即布局近似 `(cap, ptr, len)`）。该偏移由
    /// `aluka-vm::jit_helpers::pic_layout` 在运行时探测填充。
    pub slots_data_off: usize,
    /// `Ordinary.deleted_gen` 字段偏移
    pub deleted_gen_off: usize,
    /// `Ordinary.has_accessors` 字段偏移
    pub has_accessors_off: usize,
    /// `Ordinary` 判别式：props 为 `Shape` 变体的判别值
    pub disc_shape: i32,
    /// `HeapObject::Closure.upvalues`（`Vec`）结构体内**数据指针**字段的
    /// 对象内偏移（运行时探测填充——`Vec` 为 repr(Rust)，见
    /// `slots_data_off` 注释）。机器直调 uses_upvalues=true 被调时经此
    /// 读取被调上值表指针换装 ctx。
    pub closure_uv_ptr_off: usize,
    /// `HeapObject::Closure.upvalues` 的 `len` 字段对象内偏移
    pub closure_uv_len_off: usize,
}

/// JIT 调用的运行时上下文（由 aluka-vm 构造并复用）。
#[repr(C)]
#[derive(Debug)]
pub struct JitCtx {
    /// VM 状态指针（helper 经其访问解释器；单线程同步调用）
    pub vm: *mut core::ffi::c_void,
    /// 当前 JIT 函数的常量池基址（helper 按 `key_idx` 取属性名字符串）
    pub consts_ptr: *const Constant,
    /// 常量池长度
    pub consts_len: usize,
    /// 堆基址（GC 非移动；`Vec` 扩容后须刷新，见模块文档）
    pub heap_ptr: *const u8,
    /// 堆对象步长（`size_of::<HeapObject>()`）
    pub heap_stride: usize,
    /// 活跃 JIT 帧计数器的地址（指向 `Vm` 自有的 `u32` 字段）。
    ///
    /// JIT 在函数入口自增、每个返回点自减（内联的两条内存 RMW，不经 helper）。
    /// **GC 安全的核心不变量**：JIT 的局部与操作数活在机器寄存器/栈上，没有
    /// 栈映射，GC 无法把它们当根扫描——因此计数 > 0 期间一律**不得回收**
    /// （见 `aluka-vm::heap::push_object`）。计数归零后的下一次分配自然补上
    /// 被推迟的回收（分配计数器在推迟期间继续累积）。
    pub frames_ptr: *mut u32,
    /// 全局变量表变更代数：全局 IC 只在代数相等时直读。
    pub globals_gen: u32,
    /// JIT 代数：模块函数表替换（`func_idx` 语义改变）时递增。
    ///
    /// [`CallCell`] 记录登记时的代数，守卫失配即回 helper 重新解析——避免
    /// 直调到已失效模块的机器码入口。
    pub jit_gen: u32,
    /// 对象布局偏移（PIC 快速路径）
    pub layout: JitLayout,
    /// helper 函数指针表
    pub vtable: JitVtable,
    /// 当前帧机器可寻址上值表数据基址（`Upvalue` 数组；null = 未安装）。
    ///
    /// `jit_run` / `emit_call` 快路径在进入被调前换装、返回后恢复；
    /// `LOAD_UPVALUE` helper 经本表读单元格（切片四：机器可寻址上值表）。
    pub upvals_ptr: *const core::ffi::c_void,
    /// 上值表长度（元素 = `Upvalue`，8 字节）
    pub upvals_len: usize,
}

impl JitCtx {
    /// 读取常量池第 `idx` 项（helper 侧安全包装）。
    ///
    /// # Safety
    /// `consts_ptr/consts_len` 必须指向存活且长度一致的 `Vec<Constant>`。
    #[must_use]
    pub unsafe fn constant(&self, idx: u32) -> Option<&Constant> {
        if (idx as usize) < self.consts_len {
            // SAFETY: 调用方保证 consts_ptr 指向长度 == consts_len 的切片
            Some(unsafe { &*self.consts_ptr.add(idx as usize) })
        } else {
            None
        }
    }
}

/// 供无 helper 需求的纯数值 JIT 调用使用的零值上下文（不读任何字段、
/// 不调用任何 helper 时安全；`call` 便捷入口使用）。
///
/// 例外：`frames_ptr` **必须**是可写的合法地址——机器码在入口/返回点无条件
/// 自增自减该计数（GC 延迟窗口，见 [`JitCtx::frames_ptr`]），零指针会当场
/// 段错误。这里指向一个进程内的废纸篓计数器：没有 VM 的调用不涉及 GC，
/// 计数值本身无人读取。
#[must_use]
pub fn zeroed_ctx() -> JitCtx {
    /// 无 VM 调用的帧计数废纸篓（值无意义，只需地址合法可写）。
    static SCRATCH_FRAMES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    // SAFETY: JitCtx 仅含原始指针 / 函数指针 / 整数，全零为合法位模式；调用方
    // 保证零值上下文不读字段、不调用 helper（见 `JittedFn::call` 文档契约）。
    let zeroed = std::mem::MaybeUninit::<JitCtx>::zeroed();
    // SAFETY: JitCtx 仅含原始指针 / 函数指针 / 整数，全零为合法位模式
    let mut ctx = unsafe { zeroed.assume_init() };
    ctx.frames_ptr = SCRATCH_FRAMES.as_ptr();
    ctx
}

// 便捷判型（供 lib.rs 代码生成侧共用，避免在 codegen 里重复魔数）。
/// 盒是否为数值。
#[must_use]
pub const fn is_number(b: u64) -> bool {
    valbox::is_number(b)
}
/// 盒是否为对象。
#[must_use]
pub const fn is_object(b: u64) -> bool {
    valbox::is_object(b)
}
