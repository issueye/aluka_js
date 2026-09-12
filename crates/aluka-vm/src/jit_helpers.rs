//! JIT helper 桥：`extern "C"` 包装 Tier 0 语义，供 JIT 机器码经
//! [`aluka_jit::ctx::JitVtable`] 回调。
//!
//! # Safety 论证（AGENTS 约束 1 的 JIT 例外）
//! - `ctx.vm` 在 JIT 调用期间指向当前 `Vm`；单线程同步契约下无其它别名，
//!   经裸指针重建 `&mut Vm` 是唯一入口；
//! - `ctx.consts_ptr/consts_len` 指向模块常驻的常量池（Rc 保活），在调用
//!   期间不变，`key_str` 按 `key_idx` 越界安全读取；
//! - 任何可能分配（触发 GC / `Vec` 扩容）的 helper 后必须 [`refresh_heap`]
//!   刷新 `ctx.heap_ptr`，JIT 侧约定每次堆访问前从 ctx 重读。
//!
//! 本模块是 AGENTS 明文允许的 FFI 边界 unsafe 例外（JIT↔VM 桥），作用域收敛
//! 在本模块内，不解除 aluka-vm 其余部分的 `unsafe_code = deny`。

#![allow(unsafe_code)]

use crate::heap::{HeapObject, OrdinaryProps};
use crate::interpreter::Vm;
use crate::ops;
use crate::value::{Upvalue, Value, ValueCase};
use aluka_bytecode::Constant;
use aluka_core::ObjectRef;
use aluka_jit::ctx::JitCtx;
use aluka_jit::valbox;
use std::rc::Rc;

/// 盒 → VM Value。
pub(crate) fn to_vm_value(b: u64) -> Value {
    if valbox::is_number(b) {
        Value::Number(valbox::unbox_number(b))
    } else {
        match b & 0xFF {
            valbox::TAG_UNDEFINED => Value::Undefined,
            valbox::TAG_NULL => Value::Null,
            valbox::TAG_FALSE => Value::Boolean(false),
            valbox::TAG_TRUE => Value::Boolean(true),
            valbox::TAG_OBJECT => Value::Object(ObjectRef(valbox::unbox_object(b))),
            _ => Value::Undefined,
        }
    }
}

/// VM Value → 盒。
pub(crate) fn from_vm_value(v: Value) -> u64 {
    match v.case() {
        ValueCase::Undefined => valbox::UNDEFINED,
        ValueCase::Null => valbox::NULL,
        ValueCase::Boolean(false) => valbox::FALSE,
        ValueCase::Boolean(true) => valbox::TRUE,
        ValueCase::Number(n) => valbox::box_number(n),
        ValueCase::Object(r) => valbox::box_object(r.0),
    }
}

/// 按索引取常量池属性名（越界返回空串）。
///
/// 返回**借自 ctx 常量池的切片**而非克隆：属性名在热路径上每轮都取，
/// `String` 克隆是可测开销。生命周期由模块文档的 ctx 不变量保证
/// （`consts_ptr` 指向的常量池在整个 JIT 调用期间存活且不移动）；`Vec<Constant>`
/// 一旦装载即只读，故切片在 helper 返回前不会失效。
fn key_str<'a>(ctx: *mut JitCtx, key_idx: u32) -> &'a str {
    // SAFETY: ctx 由调用方填充，consts_ptr/consts_len 保证常量池存活且长度一致
    match unsafe { (*ctx).constant(key_idx) } {
        Some(Constant::String(s)) => s.as_str(),
        _ => "",
    }
}

/// ctx 常量池切片（`ops::eq` 等需要整表的语义）。
///
/// 取 **JIT 函数自己的**常量池而非 `vm.current_constants`：原生直调
/// （JIT→JIT）不切换解释器帧字段，`current_constants` 仍是调用方的池，
/// 用它会让被调的相等语义读到别的函数的常量表。
fn ctx_constants<'a>(ctx: *mut JitCtx) -> &'a [Constant] {
    // SAFETY: 见模块文档；空指针（zeroed_ctx）时返回空切片
    unsafe {
        let p = (*ctx).consts_ptr;
        if p.is_null() {
            &[]
        } else {
            std::slice::from_raw_parts(p, (*ctx).consts_len)
        }
    }
}

/// 刷新 ctx 堆基址（helper 可能分配触发 GC / Vec 扩容后必须调用）。
fn refresh_heap(ctx: *mut JitCtx, vm: &Vm) {
    // SAFETY: ctx 在调用栈上，堆基址随 Vm.heap 实时更新
    unsafe { (*ctx).heap_ptr = vm.heap.as_ptr() as *const u8 }
}

/// 属性读取（全语义：访问器/原型链/数组/字符串接收者）；回写 PIC 缓存：
/// 对象为 Shape 模式且无删除/无访问器且含该键 → (shape_id, slot, FAST)；
/// 否则 → (…, NO_FAST)。
///
/// # Safety
/// 见模块文档；`cell` 指向 JIT 编写的可写内联缓存单元。
pub unsafe extern "C" fn jit_get_property(
    ctx: *mut JitCtx,
    obj: u64,
    key_idx: u32,
    cell: *mut aluka_jit::ctx::PicCell,
) -> u64 {
    // SAFETY: 单线程同步调用，ctx.vm 指向当前 Vm
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let key = key_str(ctx, key_idx);
    let val = match vm.get_property(to_vm_value(obj), key) {
        Ok(v) => v,
        Err(_) => Value::Undefined,
    };
    // 失配回写：委托给通用缓存更新逻辑
    pic_writeback(vm, to_vm_value(obj), key, cell);
    refresh_heap(ctx, vm);
    from_vm_value(val)
}

/// 属性写入（全语义）；返回被写值（对齐 `SET_PROP` 压回）；回写 PIC 缓存。
///
/// # Safety
/// 见模块文档；`cell` 指向 JIT 编写的可写内联缓存单元。
pub unsafe extern "C" fn jit_set_property(
    ctx: *mut JitCtx,
    obj: u64,
    key_idx: u32,
    val: u64,
    cell: *mut aluka_jit::ctx::PicCell,
) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let key = key_str(ctx, key_idx);
    let _ = vm.set_property(to_vm_value(obj), key, to_vm_value(val));
    pic_writeback(vm, to_vm_value(obj), key, cell);
    refresh_heap(ctx, vm);
    val
}

/// PIC 缓存回写：对象为（Shape 模式 && 无删除 && 无访问器 && 含该键）→
/// 记录 (shape_id, slot, FAST)；否则 NO_FAST。
fn pic_writeback(vm: &mut Vm, obj: Value, key: &str, cell: *mut aluka_jit::ctx::PicCell) {
    if let Some(r) = obj.as_object() {
        let idx = r.0 as usize;
        if let Some(HeapObject::Ordinary {
            props,
            deleted,
            deleted_gen,
            has_accessors,
            ..
        }) = vm.heap.get(idx)
        {
            if *deleted_gen == 0
                && *has_accessors == 0
                && !deleted.contains(key)
                && matches!(props, OrdinaryProps::Shape { .. })
            {
                if let OrdinaryProps::Shape { shape, slots } = props {
                    // 先按 shape 查槽位；命中则记 FAST，否则保守 NO_FAST
                    let slot = vm.shape_table.shape(*shape).and_then(|s| s.lookup(key));
                    if let Some(slot) = slot {
                        if slot < slots.len() {
                            // SAFETY: cell 由 JIT 提供、单线程独占、本次调用存活
                            unsafe {
                                *cell = aluka_jit::ctx::PicCell {
                                    shape_id: shape.0,
                                    slot: slot as u32,
                                    state: aluka_jit::ctx::PicCell::FAST,
                                };
                            }
                            return;
                        }
                    }
                }
            }
        }
    }
    // SAFETY: cell 由 JIT 提供、单线程独占、本次调用存活
    unsafe { (*cell).state = aluka_jit::ctx::PicCell::NO_FAST }
}

/// 分配空普通对象（`NEW_OBJECT` 0 属性形态）。
///
/// # Safety
/// 见模块文档。
pub unsafe extern "C" fn jit_alloc_ordinary(ctx: *mut JitCtx) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let r = vm.alloc_ordinary();
    refresh_heap(ctx, vm);
    from_vm_value(Value::Object(r))
}

/// `ADD` 全语义（字符串拼接 / ToPrimitive，见 VM `add_values`）。
///
/// # Safety
/// 见模块文档。
pub unsafe extern "C" fn jit_add(ctx: *mut JitCtx, a: u64, b: u64) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let r = vm.add_values(to_vm_value(a), to_vm_value(b));
    refresh_heap(ctx, vm);
    from_vm_value(r)
}

/// `EQ` 非严格相等（全语义）。
///
/// # Safety
/// 见模块文档。
pub unsafe extern "C" fn jit_eq(ctx: *mut JitCtx, a: u64, b: u64) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let r = ops::eq(to_vm_value(a), to_vm_value(b), &vm.heap, ctx_constants(ctx));
    from_vm_value(Value::Boolean(r))
}

/// `STRICT_EQ`（全语义）。
///
/// # Safety
/// 见模块文档。
pub unsafe extern "C" fn jit_strict_eq(ctx: *mut JitCtx, a: u64, b: u64) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let r = ops::strict_eq(to_vm_value(a), to_vm_value(b), &vm.heap, ctx_constants(ctx));
    from_vm_value(Value::Boolean(r))
}

/// `ToNumber`（全语义）。
///
/// # Safety
/// 见模块文档。
pub unsafe extern "C" fn jit_to_number(_ctx: *mut JitCtx, a: u64) -> u64 {
    valbox::box_number(ops::to_number(to_vm_value(a)))
}

/// `ToBoolean`（全语义）。
///
/// # Safety
/// 见模块文档。
pub unsafe extern "C" fn jit_to_boolean(_ctx: *mut JitCtx, a: u64) -> u64 {
    // JIT 路径无堆访问：字符串按非空判定无法在此完成（传空堆，
    // 字符串回退 truthy）；JIT 化仅覆盖无字符串条件的基本块。
    from_vm_value(Value::Boolean(ops::to_boolean(to_vm_value(a), &[])))
}

impl Vm {
    /// 构造 JIT 调用上下文（每次 JIT 调用在栈上构造一次）。
    ///
    /// 活跃 JIT 帧数和代数之外，全局 epoch 每次入口刷新，防公开 HashMap 外部修改
    /// 绕过失效机制。
    pub fn build_jit_ctx(&mut self, consts: &Rc<Vec<Constant>>) -> JitCtx {
        JitCtx {
            vm: self as *mut Vm as *mut core::ffi::c_void,
            consts_ptr: consts.as_ptr(),
            consts_len: consts.len(),
            heap_ptr: self.heap.as_ptr() as *const u8,
            heap_stride: std::mem::size_of::<HeapObject>(),
            frames_ptr: &mut self.jit_frames as *mut u32,
            globals_gen: self.globals_epoch32(),
            jit_gen: self.jit_gen,
            layout: pic_layout(),
            vtable: aluka_jit::ctx::JitVtable {
                call_method: jit_call_method,
                construct: jit_construct,
                call_args: jit_call_args,
                call_this: jit_call_this,
                get_property: jit_get_property,
                set_property: jit_set_property,
                alloc_ordinary: jit_alloc_ordinary,
                add: jit_add,
                eq: jit_eq,
                strict_eq: jit_strict_eq,
                to_number: jit_to_number,
                to_boolean: jit_to_boolean,
                call: jit_call,
                load_global: jit_load_global,
                load_upvalue: jit_load_upvalue,
                typeof_: jit_typeof,
                typeof_global: jit_typeof_global,
                get_elem: jit_get_elem,
                set_elem: jit_set_elem,
                del_prop: jit_del_prop,
                get_proto: jit_get_proto,
                instanceof: jit_instanceof,
                in_: jit_in,
                new_array: jit_new_array,
                array_push: jit_array_push,
            },
            upvals_ptr: std::ptr::null(),
            upvals_len: 0,
        }
    }
}

/// 调用（`CALL`）：实参数组来自 JIT 栈槽，语义交给解释器 `invoke_callable`
/// （原生函数、闭包、Promise resolver 等全部路径一致）。
///
/// 同时**回写本站点的调用内联缓存**：被调是「无上值闭包 + 已编译 + 编译产物
/// 不读上值 + 实参数量够形参」时登记入口地址与常量池，下轮由 JIT 直接
/// `call_indirect`，不再进本 helper。
///
/// # Safety
/// 见模块文档；`args_ptr` 指向 `argc` 个连续 u64 盒（JIT 栈槽，调用期有效）。
pub unsafe extern "C" fn jit_call(
    ctx: *mut JitCtx,
    callee: u64,
    args_ptr: *const u64,
    argc: u32,
    cell: *mut aluka_jit::ctx::CallCell,
) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    vm.jit_call_fallbacks += 1;
    // ≤8 实参走栈数组（避免每次 CALL 的堆分配）
    let mut inline = [Value::Undefined; 8];
    let n = argc as usize;
    let heap_args: Vec<Value>;
    let args: &[Value] = if n <= 8 {
        for (i, slot) in inline.iter_mut().enumerate().take(n) {
            // SAFETY: 调用方保证 args_ptr 指向 argc 个连续 u64
            *slot = to_vm_value(unsafe { *args_ptr.add(i) });
        }
        &inline[..n]
    } else {
        heap_args = (0..n)
            .map(|i| {
                // SAFETY: 同上
                to_vm_value(unsafe { *args_ptr.add(i) })
            })
            .collect();
        &heap_args
    };
    // 调用 IC 回写：判定被调可否原生直调（判定只做一次，之后由 JIT 侧守卫）
    // SAFETY: cell 由 JIT 提供、单线程独占、本次调用存活
    let cur_gen = unsafe { (*ctx).jit_gen };
    call_ic_writeback(vm, callee, n, cur_gen, cell);
    // 快速路径：被调是**已编译且无上值**的闭包 → 直接跑机器码，跳过
    // `invoke_callable`/`invoke_function` 的解释器帧建立（locals Vec 分配、
    // constants/upvalues/try 表保存恢复）。有上值或未编译时走通用路径。
    if let Some(direct) = vm.jit_direct_call(to_vm_value(callee), args) {
        refresh_heap(ctx, vm);
        return from_vm_value(direct);
    }
    let r = match vm.invoke_callable(to_vm_value(callee), Value::Undefined, args) {
        Ok(v) => v,
        Err(_) => Value::Undefined,
    };
    refresh_heap(ctx, vm);
    from_vm_value(r)
}

/// 调用 IC 回写：被调符合原生直调条件时登记 (入口, 常量池, 代数, FAST)。
///
/// 直调条件（缺一不可，否则 `NO_FAST` 永久回退本 helper）：
/// 1. 被调是**闭包对象且无上值**——直调不建解释器帧，无处安装上值表；
/// 2. 该函数已 JIT 编译（`JitSlot::Compiled`）；
/// 3. 编译产物不读上值（`uses_upvalues == false`）——同上；
/// 4. 实参数量 ≥ 形参数量——JIT 侧按形参数读 `args_ptr`，少传会读到邻位垃圾；
/// 5. 被调不是 varargs / 需 `arguments`（这两类由资格判定拦在编译前，
///    编译成功即已排除）。
fn call_ic_writeback(
    vm: &mut Vm,
    callee: u64,
    argc: usize,
    cur_gen: u32,
    cell: *mut aluka_jit::ctx::CallCell,
) {
    let no_fast = |cell: *mut aluka_jit::ctx::CallCell| {
        // SAFETY: cell 由 JIT 提供、单线程独占、本次调用存活
        unsafe { (*cell).state = aluka_jit::ctx::CallCell::NO_FAST }
    };
    // NO_FAST 短路：同站点同被调已永久判定不可直调（机器守卫按位比对
    // callee，被调变化时位不同自然重判），避免每次调用重复完整判定
    //（递归调用密集负载上 writeback 本身是每调用开销）

    let ValueCase::Object(r) = to_vm_value(callee).case() else {
        return no_fast(cell);
    };
    // 直调资格 = 编译产物不读上值（jit_entry_for 的 uses_upvalues 守卫）；
    // 闭包捕获了哪些单元格无关——机器码不含 LoadUpvalue 就不会碰调用方
    // 安装的上值表（切片四 P1：upvalues.is_empty() 判定曾把 Go/Rust 前端
    // 顶层函数全部排除在直调之外，fib30 269 万次调用全走 helper 回退）
    let func_idx = match vm.heap.get(r.0 as usize) {
        Some(HeapObject::Closure { func_idx, .. }) => *func_idx,
        _ => return no_fast(cell),
    };
    let Some(entry) = vm.jit_entry_for(func_idx) else {
        // 尚未编译（或已被拒 / 编译产物读上值）：本次不登记
        return no_fast(cell);
    };
    let Some(tmpl) = vm.module_functions.get(func_idx) else {
        return no_fast(cell);
    };
    if argc < tmpl.num_params as usize {
        return no_fast(cell);
    }
    let Some(consts) = vm.module_constants.get(func_idx) else {
        return no_fast(cell);
    };
    // SAFETY: cell 由 JIT 提供、单线程独占、本次调用存活
    unsafe {
        *cell = aluka_jit::ctx::CallCell {
            callee,
            entry,
            consts_ptr: consts.as_ptr() as usize,
            consts_len: consts.len(),
            cell_gen: cur_gen,
            state: aluka_jit::ctx::CallCell::FAST,
            // 上值表指针不缓存：emit_call 快路径每次从被调堆对象现读
            // （对象存活期间 Vec 缓冲区地址稳定，无陈旧指针问题）
            upvals_ptr: 0,
            upvals_len: 0,
        };
    }
}

/// 全局读取（`LOAD_GLOBAL`）。命中公开 `Vm::globals` 的真实键时回写 Global IC；
/// 动态内建与缺失键标为 `NO_FAST`，保持原有完整解析语义。
///
/// # Safety
/// 见模块文档；`cell` 指向 JIT 编写的可写 Global IC 单元。
pub unsafe extern "C" fn jit_load_global(
    ctx: *mut JitCtx,
    name_idx: u32,
    cell: *mut aluka_jit::ctx::GlobalCell,
) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let name = key_str(ctx, name_idx);
    let epoch = vm.globals_epoch32();
    if let Some(value) = vm.globals.get(name).copied() {
        // 只有公开 globals 中的真实键可安全缓存；动态内建仍需走 resolve_global。
        // SAFETY: cell 由 JIT 提供、单线程独占；当前调用期间地址稳定且可写。
        unsafe {
            *cell = aluka_jit::ctx::GlobalCell {
                key_idx: name_idx,
                state: aluka_jit::ctx::GlobalCell::FAST,
                globals_gen: epoch,
                _pad: 0,
                value: from_vm_value(value),
            };
        }
        return from_vm_value(value);
    }
    // 缺失键和动态内建不缓存：每次 helper 仍复刻解释器的动态解析语义。
    // SAFETY: cell 由 JIT 提供、单线程独占；当前调用期间地址稳定且可写。
    unsafe {
        (*cell).state = aluka_jit::ctx::GlobalCell::NO_FAST;
    }
    let v = vm.resolve_global_for_jit(name);
    refresh_heap(ctx, vm);
    from_vm_value(v)
}

/// 上值读取（`LOAD_UPVALUE`）：读当前帧上值表（`jit_run` 已安装）。
///
/// # Safety
/// 见模块文档。
pub unsafe extern "C" fn jit_load_upvalue(ctx: *mut JitCtx, uv_idx: u32) -> u64 {
    // 切片四：优先读机器可寻址上值表（emit_call 快路径 / jit_run 换装，
    // 指向被调闭包自己的单元格——递归自引用等体读上值的被调由此正确）；
    // 表未安装时回退解释器帧的 current_upvalues。
    // SAFETY: ctx 由 JIT 同步传入（见模块文档），字段读取在 unsafe fn 内
    let (ptr, len) = unsafe { ((*ctx).upvals_ptr, (*ctx).upvals_len) };
    let idx = uv_idx as usize;
    let v = if !ptr.is_null() && idx < len {
        // SAFETY: 表在调用期间存活——单元格由被调闭包持有（调用中被根集合
        // /延迟回收保护），表缓冲区由 current_upvalues 或闭包对象持有
        let uv = unsafe { &*(ptr as *const Upvalue).add(idx) };
        *uv.0.borrow()
    } else {
        // SAFETY: 见模块文档
        let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
        vm.current_upvalues
            .get(idx)
            .map(|uv| *uv.0.borrow())
            .unwrap_or(Value::Undefined)
    };
    from_vm_value(v)
}

/// 测量 Ordinary 对象 PIC 快速路径所需字段偏移（引用差一次成型）。
///
/// `shape_id_off` / `slots_ptr_off` 是相对 `OrdinaryProps::Shape` 变体头部的
/// 偏移（JIT 侧叠加 `layout.props_off`）。`disc_shape` 为 `#[repr(C)]` 下
/// `OrdinaryProps` 判别式的原始值（Shape 变体 = 0）。
#[must_use]
pub fn pic_layout() -> aluka_jit::ctx::JitLayout {
    // 布局在进程生命周期内固定：缓存后每次 JIT 调用只读一次静态值
    // （否则每次 `build_jit_ctx` 都要构造探针对象 + 扫描 Vec 字段，
    //  在 closureCall 这类每轮都进 JIT 的负载上是主开销）。
    static CACHE: std::sync::OnceLock<aluka_jit::ctx::JitLayout> = std::sync::OnceLock::new();
    *CACHE.get_or_init(measure_pic_layout)
}

/// 实测对象布局（仅由 [`pic_layout`] 首次调用触发）。
fn measure_pic_layout() -> aluka_jit::ctx::JitLayout {
    use crate::heap::HeapObject;
    use aluka_core::ShapeId;
    use std::collections::{HashMap, HashSet};
    let probe = HeapObject::Ordinary {
        props: OrdinaryProps::Shape {
            shape: ShapeId(0x1234_5678),
            slots: vec![aluka_jit::valbox::UNDEFINED, aluka_jit::valbox::UNDEFINED],
        },
        getters: HashMap::new(),
        setters: HashMap::new(),
        proto: None,
        deleted: HashSet::new(),
        non_enum: HashSet::new(),
        deleted_gen: 0,
        has_accessors: 0,
    };
    let HeapObject::Ordinary {
        props,
        deleted_gen,
        has_accessors,
        ..
    } = &probe
    else {
        unreachable!("版颈必须是 Ordinary")
    };
    let base = (&probe as *const HeapObject) as usize;
    let props_off = (props as *const OrdinaryProps as usize) - base;
    let deleted_gen_off = (deleted_gen as *const u32 as usize) - base;
    let has_accessors_off = (has_accessors as *const u32 as usize) - base;
    let OrdinaryProps::Shape { shape, slots } = props else {
        unreachable!("版颈必须是 Shape 变体")
    };
    let props_base = props as *const OrdinaryProps as usize;
    let shape_id_off = (shape as *const ShapeId as usize) - props_base;
    let slots_ptr_off = (slots as *const Vec<u64> as usize) - props_base;
    // `Vec` 是 repr(Rust)：字段顺序不受保证。数据指针**在 Vec 结构体内某一
    // 机器字**，运行时扫描三个字找出等于 `as_ptr()` 的那个（`as_ptr()` 本身
    // 返回堆上数据地址，不是结构体内偏移——直接相减会得到垃圾值）。
    let slots_data_off = {
        let vec_base = slots as *const Vec<u64> as usize;
        let want = slots.as_ptr() as usize;
        let mut found = usize::MAX;
        for i in 0..(std::mem::size_of::<Vec<u64>>() / 8) {
            // SAFETY: 读取自有 Vec 结构体内的机器字（探测字段布局）
            let w = unsafe { *((vec_base + i * 8) as *const usize) };
            if w == want {
                found = i * 8;
                break;
            }
        }
        assert!(
            found != usize::MAX,
            "未能在 Vec 结构体内定位数据指针字段——布局假设失效"
        );
        found
    };
    // SAFETY: repr(C) 枚举判别式固定为偏移 0 的 i32（普通对象属性存储契约）
    let disc_shape = unsafe { *(props as *const OrdinaryProps as *const i32) };

    // Closure.upvalues 表指针/长度偏移探测（切片四：机器可寻址上值表）。
    // with_capacity(4) + push 3 个单元格 → len=3、cap=4，两个值可区分：
    // 数据指针字 = as_ptr()；len 字 = 3；cap 字 = 4。
    let mut uv_probe: Vec<Upvalue> = Vec::with_capacity(4);
    for n in 0..3i32 {
        uv_probe.push(Upvalue(std::rc::Rc::new(std::cell::RefCell::new(
            Value::Number(f64::from(n)),
        ))));
    }
    let closure_probe = HeapObject::Closure {
        func_idx: 0,
        upvalues: uv_probe,
        properties: HashMap::new(),
        getters: HashMap::new(),
        non_enum: HashSet::new(),
        proto: None,
    };
    let (closure_uv_ptr_off, closure_uv_len_off) = {
        let HeapObject::Closure { upvalues, .. } = &closure_probe else {
            unreachable!("探针必须是 Closure")
        };
        let base = (&closure_probe as *const HeapObject) as usize;
        let vec_base = (upvalues as *const Vec<Upvalue>) as usize;
        let data_ptr = upvalues.as_ptr() as usize;
        let mut ptr_off = usize::MAX;
        let mut len_off = usize::MAX;
        for i in 0..(std::mem::size_of::<Vec<Upvalue>>() / 8) {
            // SAFETY: 读取自有 Vec 结构体内的机器字（探测字段布局）
            let w = unsafe { *((vec_base + i * 8) as *const usize) };
            if w == data_ptr {
                ptr_off = i * 8;
            } else if w == 3 {
                len_off = i * 8;
            }
        }
        assert!(
            ptr_off != usize::MAX && len_off != usize::MAX,
            "未能在 Closure.upvalues 内定位表指针/长度字段——布局假设失效"
        );
        // 字偏移（Vec 内）+ Vec 结构体在堆对象内的偏移 = 对象内总偏移
        let closure_uv_ptr_off = vec_base + ptr_off - base;
        let closure_uv_len_off = vec_base + len_off - base;
        (closure_uv_ptr_off, closure_uv_len_off)
    };
    aluka_jit::ctx::JitLayout {
        props_off,
        shape_id_off,
        slots_ptr_off,
        slots_data_off,
        deleted_gen_off,
        has_accessors_off,
        disc_shape,
        closure_uv_ptr_off,
        closure_uv_len_off,
    }
}

/// `NEW`：callee+实参表 → 解释器 `do_construct`。
///
/// 偏离登记（J2 既有约定，与 [`jit_call`] 一致）：helper 返回通道无错误面，
/// 构造内抛错归一为 undefined。
///
/// # Safety
/// 见模块文档：`ctx` 由 JIT 同步传入且独占当前 `Vm`，`args_ptr` 指向
/// `argc` 个连续 NaN-box 机器字。
pub unsafe extern "C" fn jit_construct(
    ctx: *mut JitCtx,
    callee: u64,
    args_ptr: *const u64,
    argc: u32,
) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let mut inline = [Value::Undefined; 8];
    let n = argc as usize;
    let heap_args: Vec<Value>;
    let args: &[Value] = if n <= 8 {
        for (i, slot) in inline.iter_mut().enumerate().take(n) {
            // SAFETY: 调用方保证 args_ptr 指向 argc 个连续 u64
            *slot = to_vm_value(unsafe { *args_ptr.add(i) });
        }
        &inline[..n]
    } else {
        heap_args = (0..n)
            .map(|i| {
                // SAFETY: 同上
                to_vm_value(unsafe { *args_ptr.add(i) })
            })
            .collect();
        &heap_args
    };
    let r = match vm.do_construct(to_vm_value(callee), args) {
        Ok(v) => v,
        Err(_) => Value::Undefined,
    };
    refresh_heap(ctx, vm);
    from_vm_value(r)
}

/// `CALL_ARGS`/`NEW_ARGS`：实参数组值 → 解释器展开（`to_array_values`）。
///
/// # Safety
/// 见模块文档：`ctx` 由 JIT 同步传入且独占当前 `Vm`。
pub unsafe extern "C" fn jit_call_args(ctx: *mut JitCtx, callee: u64, args_array: u64) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let args = vm.to_array_values(to_vm_value(args_array));
    let r = match vm.invoke_callable(to_vm_value(callee), Value::Undefined, &args) {
        Ok(v) => v,
        Err(_) => Value::Undefined,
    };
    refresh_heap(ctx, vm);
    from_vm_value(r)
}

/// `CALL_METHOD`：receiver+方法名+实参 → 解释器统一分派链
/// （`call_method_dispatch`，内建内联分派与解释器 `Op::CallMethod` 单源）。
///
/// 偏离登记（J2 既有约定，与 [`jit_call`] 一致）：helper 返回通道无错误面，
/// 分派内抛错归一为 undefined。
///
/// # Safety
/// 见模块文档：`ctx` 由 JIT 同步传入且独占当前 `Vm`，`args_ptr` 指向
/// `argc` 个连续 NaN-box 机器字。
pub unsafe extern "C" fn jit_call_method(
    ctx: *mut JitCtx,
    receiver: u64,
    name_idx: u32,
    args_ptr: *const u64,
    argc: u32,
) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let name = key_str(ctx, name_idx);
    let mut inline = [Value::Undefined; 8];
    let n = argc as usize;
    let heap_args: Vec<Value>;
    let args: &[Value] = if n <= 8 {
        for (i, slot) in inline.iter_mut().enumerate().take(n) {
            // SAFETY: 调用方保证 args_ptr 指向 argc 个连续 u64
            *slot = to_vm_value(unsafe { *args_ptr.add(i) });
        }
        &inline[..n]
    } else {
        heap_args = (0..n)
            .map(|i| {
                // SAFETY: 同上
                to_vm_value(unsafe { *args_ptr.add(i) })
            })
            .collect();
        &heap_args
    };
    // 方法 IC：JIT 站点共享固定站点键 u64::MAX（与解释器站点键空间不相交；
    // 单态热点即享原型绑定缓存，多态互挤回退慢路径，语义仍正确）
    let r = match vm.call_method_dispatch(to_vm_value(receiver), name, args, u64::MAX) {
        Ok(v) => v,
        Err(_) => Value::Undefined,
    };
    refresh_heap(ctx, vm);
    from_vm_value(r)
}

/// `CALL_WITH_THIS`/`CALL_WITH_THIS_ARGS`：显式 this 调用。
///
/// 两变体共用一个 helper：`argc > 0` 时 `args_ptr_or_array` 按实参表指针
/// 解释（`argc` 个连续机器字）；`argc == 0` 时按数组值解释（展开）。
///
/// # Safety
/// 见模块文档：`ctx` 由 JIT 同步传入且独占当前 `Vm`。
pub unsafe extern "C" fn jit_call_this(
    ctx: *mut JitCtx,
    callee: u64,
    this_val: u64,
    args_ptr_or_array: u64,
    argc: u32,
) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let n = argc as usize;
    let ret = if n > 0 {
        // SAFETY: argc>0 约定下该参数为 JIT 传入的实参表指针
        let ptr = args_ptr_or_array as *const u64;
        let mut inline = [Value::Undefined; 8];
        let heap_args: Vec<Value>;
        let args: &[Value] = if n <= 8 {
            for (i, slot) in inline.iter_mut().enumerate().take(n) {
                // SAFETY: 调用方保证指针指向 argc 个连续 u64
                *slot = to_vm_value(unsafe { *ptr.add(i) });
            }
            &inline[..n]
        } else {
            heap_args = (0..n)
                .map(|i| {
                    // SAFETY: 同上
                    to_vm_value(unsafe { *ptr.add(i) })
                })
                .collect();
            &heap_args
        };
        vm.invoke_callable(to_vm_value(callee), to_vm_value(this_val), args)
    } else {
        let arr = vm.to_array_values(to_vm_value(args_ptr_or_array));
        vm.invoke_callable(to_vm_value(callee), to_vm_value(this_val), &arr)
    };
    let r = match ret {
        Ok(v) => v,
        Err(_) => Value::Undefined,
    };
    refresh_heap(ctx, vm);
    from_vm_value(r)
}

/// `TYPEOF`：typeof 语义（解释器 `typeof_value` 单源），返回字符串对象盒。
///
/// # Safety
/// 见模块文档：`ctx` 由 JIT 同步传入且独占当前 `Vm`。
pub unsafe extern "C" fn jit_typeof(ctx: *mut JitCtx, v: u64) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let s = vm.typeof_value(to_vm_value(v));
    let r = Value::Object(vm.alloc_string(s));
    refresh_heap(ctx, vm);
    from_vm_value(r)
}

/// `TYPEOF_GLOBAL`：全局 typeof（动态解析不缓存）。
///
/// # Safety
/// 见模块文档。
pub unsafe extern "C" fn jit_typeof_global(ctx: *mut JitCtx, name_idx: u32) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let name = key_str(ctx, name_idx);
    let v = vm.resolve_global(name);
    let s = vm.typeof_value(v);
    let r = Value::Object(vm.alloc_string(s));
    refresh_heap(ctx, vm);
    from_vm_value(r)
}

/// `GET_ELEM`：动态键属性读取（`to_property_key` + `get_property` 全语义）。
///
/// # Safety
/// 见模块文档。
pub unsafe extern "C" fn jit_get_elem(ctx: *mut JitCtx, obj: u64, key: u64) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let k = vm.to_property_key(to_vm_value(key));
    let r = match vm.get_property(to_vm_value(obj), &k) {
        Ok(v) => v,
        Err(_) => Value::Undefined,
    };
    refresh_heap(ctx, vm);
    from_vm_value(r)
}

/// `SET_ELEM`：动态键属性写入，返回被写值。
///
/// # Safety
/// 见模块文档。
pub unsafe extern "C" fn jit_set_elem(ctx: *mut JitCtx, obj: u64, key: u64, val: u64) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let k = vm.to_property_key(to_vm_value(key));
    let _ = vm.set_property(to_vm_value(obj), &k, to_vm_value(val));
    refresh_heap(ctx, vm);
    val
}

/// `DEL_PROP`：自有属性删除（push `true` 面）。
///
/// # Safety
/// 见模块文档。
pub unsafe extern "C" fn jit_del_prop(ctx: *mut JitCtx, obj: u64, name_idx: u32) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let name = key_str(ctx, name_idx);
    vm.delete_property(to_vm_value(obj), name);
    from_vm_value(Value::Boolean(true))
}

/// `GET_PROTO`：`[[Prototype]]` 读取（无原型返回 null）。
///
/// # Safety
/// 见模块文档。
pub unsafe extern "C" fn jit_get_proto(ctx: *mut JitCtx, obj: u64) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let r = match vm.get_prototype(to_vm_value(obj)) {
        Some(p) => Value::Object(p),
        None => Value::Null,
    };
    refresh_heap(ctx, vm);
    from_vm_value(r)
}

/// `INSTANCEOF`：`check_instanceof` 全语义（bool 盒）。
///
/// # Safety
/// 见模块文档。
pub unsafe extern "C" fn jit_instanceof(ctx: *mut JitCtx, l: u64, r: u64) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let b = vm.check_instanceof(to_vm_value(l), to_vm_value(r));
    from_vm_value(Value::Boolean(b))
}

/// `IN`：`key in obj`（`has_property` 全语义，bool 盒）。
///
/// # Safety
/// 见模块文档。
pub unsafe extern "C" fn jit_in(ctx: *mut JitCtx, key: u64, obj: u64) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let k = vm.to_property_key(to_vm_value(key));
    let b = vm.has_property(to_vm_value(obj), &k);
    from_vm_value(Value::Boolean(b))
}

/// `NEW_ARRAY`/`BUILD_ARRAY`：n 个盒（栈数组）→ 数组对象。
///
/// # Safety
/// 见模块文档：`vals_ptr` 指向 `n` 个连续 u64 盒。
pub unsafe extern "C" fn jit_new_array(ctx: *mut JitCtx, vals_ptr: *const u64, n: u32) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let elements = (0..n as usize)
        .map(|i| {
            // SAFETY: 调用方保证 vals_ptr 指向 n 个连续 u64
            to_vm_value(unsafe { *vals_ptr.add(i) })
        })
        .collect();
    let r = Value::Object(vm.alloc_array(elements));
    refresh_heap(ctx, vm);
    from_vm_value(r)
}

/// `ARRAY_PUSH`：追加元素到数组对象（含写屏障），返回被追加值。
///
/// # Safety
/// 见模块文档。
pub unsafe extern "C" fn jit_array_push(ctx: *mut JitCtx, arr: u64, val: u64) -> u64 {
    // SAFETY: 见模块文档
    let vm = unsafe { &mut *((*ctx).vm as *mut Vm) };
    let v = to_vm_value(val);
    if let Some(r) = to_vm_value(arr).as_object() {
        if let Some(HeapObject::Array { elements, .. }) = vm.heap.get_mut(r.0 as usize) {
            elements.push(v);
        }
        vm.gc_write_barrier(r, v);
    }
    refresh_heap(ctx, vm);
    val
}

#[cfg(test)]
mod pic_layout_tests {
    use super::*;
    use crate::heap::HeapObject;
    use std::collections::HashMap;

    /// repr(C) 判别式自检：Shape=0、Dict=1，字段偏移为合理值且互不重叠。
    #[test]
    fn pic_layout_matches_repr_c_enum() {
        // Shape 探针（已在 pic_layout 内验证）；另测 Dict 探针判别式
        let dict_probe = HeapObject::Ordinary {
            props: OrdinaryProps::Dict {
                properties: Vec::new(),
                index: HashMap::new(),
            },
            getters: HashMap::new(),
            setters: HashMap::new(),
            proto: None,
            deleted: std::collections::HashSet::new(),
            non_enum: std::collections::HashSet::new(),
            deleted_gen: 0,
            has_accessors: 0,
        };
        let HeapObject::Ordinary {
            props: dict_props, ..
        } = &dict_probe
        else {
            unreachable!()
        };
        // SAFETY: repr(C) 判别式读取（自检断言其值）
        let dict_disc = unsafe { *(dict_props as *const OrdinaryProps as *const i32) };
        assert_eq!(dict_disc, 1, "Dict 变体判别式应为 1");

        let l = pic_layout();
        assert_eq!(l.disc_shape, 0, "Shape 变体判别式应为 0");
        assert!(
            l.shape_id_off >= 4 && l.shape_id_off < 16,
            "ShapeId 应在判别式（≥4 字节）之后：{}",
            l.shape_id_off
        );
        assert!(
            l.slots_ptr_off > l.shape_id_off && l.slots_ptr_off < 32,
            "slots Vec 数据指针应在 ShapeId 之后：{}",
            l.slots_ptr_off
        );
        assert!(
            l.deleted_gen_off > l.slots_ptr_off + 24,
            "deleted_gen 在 props 之后"
        );
        assert!(
            l.has_accessors_off > l.deleted_gen_off,
            "has_accessors 在 deleted_gen 之后"
        );
        // props 字段偏移应小于整个对象任何尾部字段（basic sanity）
        assert!(l.props_off < l.deleted_gen_off);
    }

    /// 自检：普通对象经 alloc_ordinary + set_property 后，JIT 布局可正确
    /// 定位 shape 与槽位（防布局测量与实际分配脱节）。
    #[test]
    fn pic_layout_agrees_with_runtime_object() {
        let mut vm = Vm::new(0);
        let o = vm.alloc_ordinary();
        let _ = vm.set_property(Value::Object(o), "x", Value::Number(4.0));
        let l = pic_layout();
        let base = vm.heap.as_ptr() as usize;
        let obj_addr = base + (o.0 as usize) * std::mem::size_of::<HeapObject>();
        let HeapObject::Ordinary { props, .. } = &vm.heap[o.0 as usize] else {
            unreachable!()
        };
        // 从布局偏移读回 props 判别式应与实际一致
        // SAFETY: 探针读取受测试保护，地址为堆内合法对象
        let disc_ptr = unsafe { (obj_addr as *const u8).add(l.props_off) as *const i32 };
        // SAFETY: 探针地址为堆内合法对象，布局偏移由同函数测量
        let disc = unsafe { *disc_ptr };
        assert_eq!(disc, l.disc_shape, "layout.props_off 判别式定位正确");
        let OrdinaryProps::Shape { shape, .. } = props else {
            unreachable!("单属性对象应仍为 Shape 变体")
        };
        // 从布局偏移读回 shape id 应与实际一致
        // SAFETY: 探针读取受测试保护
        let shape_addr =
            unsafe { (obj_addr as *const u8).add(l.props_off + l.shape_id_off) } as *const u32;
        // SAFETY: 探针地址为堆内合法对象，布局偏移由同函数测量
        let shape_id = unsafe { *shape_addr };
        assert_eq!(shape_id, shape.0, "layout.shape_id_off 定位正确");
    }
}
