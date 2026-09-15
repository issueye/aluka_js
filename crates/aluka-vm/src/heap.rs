//! 虚拟机堆内托管对象与分配器实现。

use crate::interpreter::Vm;
use crate::value::{Upvalue, Value, ValueCase};
use aluka_core::{ObjectRef, ShapeId};
use std::collections::{HashMap, HashSet};

/// 快速模式转字典模式的属性数阈值。
///
/// shape transition 树的每次派生会克隆父 shape 的完整 `names + index` 前缀，
/// 对「海量键」对象（如 Buffer 的数值下标 0..N）会退化为 O(N²) 时间与内存。
/// 属性数达到本阈值后对象整体转为字典模式（HashMap），避免 shape 爆炸。
/// 小对象（PIC 目标）保持在快速模式，属性访问仍可退化为槽位直读。
pub(crate) const DICT_THRESHOLD: usize = 32;

/// 普通对象属性存储模式。
///
/// `#[repr(C)]`：判别式固定为 i32 且位于偏移 0（Shape=0, Dict=1），JIT PIC
/// 快速路径按此判别对象是否可走槽位直读（`layout.disc_shape`）。
#[repr(C)]
#[derive(Debug, Clone)]
pub enum OrdinaryProps {
    /// 快速模式：隐藏类 + 槽位数组（结构相同的对象共享 shape，访问 O(1)）。
    ///
    /// 槽位存 **NaN-box u64 盒**（`aluka_jit::valbox`）而非 VM Value 枚举——
    /// JIT PIC 快速路径按机器字直读槽位；解释器在存取边界经
    /// `jit_helpers::to_vm_value/from_vm_value` 转换。
    Shape {
        /// 隐藏类 id（属性名 → 槽位下标映射经 `Vm::shape_table` 查询）
        shape: ShapeId,
        /// 属性槽位数组（长度 == shape 属性数；删除的槽清为 undefined 盒）
        slots: Vec<u64>,
    },
    /// 字典模式：有序键值对（超阈值后转入）
    ///
    /// `Vec<(String, Value)>` 保持**插入序**——`JSON.stringify` / `Object.keys`
    /// / `for...in` 的输出顺序与 Node 22 一致（V8 对象键序 = 创建序；
    /// 整数索引键由消费端按规范前置升序）。伴生 `index` 提供键 → 槽位下标
    /// O(1) 命中，避免顺序写入退化为 O(N²)（海量键对象如 Buffer 的 0..N
    /// 数值下标；每次 push 后同步登记，delete 移除后整体重建）。
    Dict {
        /// 有序属性列表（键 → 值；删除经 retain，重加追加末尾）
        properties: Vec<(String, Value)>,
        /// 键 → properties 槽位下标（与列表严格同步；retain 后重建）
        index: HashMap<String, usize>,
    },
}

/// 虚拟机堆内托管对象。
#[derive(Debug, Clone)]
pub enum HeapObject {
    /// 普通对象：属性存储（快速 shape+slots 或字典 HashMap）+ 访问器映射 +
    /// 删除集 + 隐式原型链。
    ///
    /// 快速模式下结构相同的对象共享同一 `shape`，属性读写退化为槽位直读/直写
    /// （JIT 内联缓存的基础，见 `docs/adr/0005`）；属性数达 [`DICT_THRESHOLD`]
    /// 后整体转字典模式。删除属性不改 shape（避免污染共享），记入
    /// `deleted` 集合并清槽、`deleted_gen` 递增作为快速路径守卫。
    Ordinary {
        /// 属性存储模式（快速 shape+slots 或字典 HashMap）
        props: OrdinaryProps,
        /// 访问器 Getter 映射表（属性名 -> 访问器函数值；存闭包对象以保留
        /// upvalue 捕获——延迟调用时闭包引用的模块/外层变量仍可解析）
        getters: HashMap<String, Value>,
        /// 访问器 Setter 映射表（属性名 -> 访问器函数值）
        setters: HashMap<String, Value>,
        /// 隐式原型 [[Prototype]]
        proto: Option<ObjectRef>,
        /// 已删除属性名集合（删除不改 shape，查询时先看本集）
        deleted: HashSet<String>,
        /// 删除代数（每次删除 +1；PIC 快速路径守卫：0 即无删除）
        deleted_gen: u32,
        /// 不可枚举属性键集合（`Object.defineProperty(.. enumerable:false)`
        /// 与原型方法面挂载；`for...in` 枚举时过滤）
        non_enum: HashSet<String>,
        /// 访问器粘性标记（0/1）：注册过 getter/setter 即置 1 永不回退，
        /// PIC 快速路径守卫（有访问器的对象不得跳过访问器语义）
        has_accessors: u32,
    },
    /// 数组对象：线性元素列表 + 自有非索引属性 + 隐式原型（`Array.prototype` 单例）
    Array {
        /// 数组内存储的值列表
        elements: Vec<Value>,
        /// 自有非索引属性（JS 数组可携带 `arr.foo` 类属性，arguments 对象也用）
        properties: HashMap<String, Value>,
        /// 隐式原型 [[Prototype]]
        proto: Option<ObjectRef>,
    },
    /// 闭包函数对象：指向所属函数模板的索引与捕获的上值
    Closure {
        /// 目标函数模板索引
        func_idx: usize,
        /// 捕获的上值列表
        upvalues: Vec<Upvalue>,
        /// 闭包自有属性（例如 prototype、静态字段等）
        properties: HashMap<String, Value>,
        /// 闭包访问器 Getter 表（`Object.defineProperty` 挂函数对象静态面，
        /// 如 body-parser 的 json/raw/text/urlencoded 惰性 getter；存闭包值）
        getters: HashMap<String, Value>,
        /// 不可枚举属性键集合（函数对象 `prototype`、`defineProperty` 未声明
        /// enumerable 的静态面——`Object.keys(fn)` 须过滤）
        non_enum: HashSet<String>,
        /// 闭包原型 [[Prototype]]（用于静态继承 superClass）
        proto: Option<ObjectRef>,
    },
    /// 堆内字符串对象（全局唯一跨函数句柄）
    String(String),
    /// 堆内 BigInt 对象（十进制字符串表示，对齐 Go 版常量池语义）
    BigInt(String),
    /// 原生构造器（Error / Array / Object 等内置构造函数，`new` 由解释器拦截求值）
    NativeCtor {
        /// 构造器名（亦为产出的错误实例 `name`）
        name: String,
        /// 构造器自有属性（如 `prototype`）
        properties: HashMap<String, Value>,
        /// 不可枚举属性键集合（规范：`prototype` 不可枚举、
        /// `Error.captureStackTrace` 不可枚举而 `stackTraceLimit` 可枚举——
        /// `Object.keys(Error)` 只列后者）
        non_enum: HashSet<String>,
    },
    /// 生成器对象（执行状态存于 `Vm.generators` 注册表，此变体仅作身份标记）
    Generator,
    /// Promise 对象（微任务队列基建后支持 then 回调调度）
    Promise {
        /// 是否已完成（fulfilled 或 rejected）
        pending: bool,
        /// 完成值（pending 时为 undefined；rejected 时为拒绝原因）
        value: Value,
        /// 是否以拒绝完成（`pending == false` 时有效）
        is_rejected: bool,
        /// 已登记的回调（`.then` 的 fulfilled 处理器，fulfill 时进微任务队列）
        handlers: Vec<Value>,
        /// `.catch` 的 rejected 处理器（reject 时调度；fulfill 不触发）
        rejected: Vec<Value>,
    },
    /// Promise 的 resolve/reject 函数（捕获目标 promise，调用即按标志 fulfill/reject）
    PromiseResolver {
        /// 被解析的目标 promise 句柄
        promise: ObjectRef,
        /// `true` = resolve（fulfill），`false` = reject
        resolve: bool,
    },
    /// EventEmitter 实例（Node `node:events`；事件名 → 监听器列表）
    EventEmitter {
        /// 事件名 → (监听器回调, 是否 once) 列表
        listeners: std::collections::HashMap<String, Vec<(Value, bool)>>,
    },
    /// Symbol 原语（唯一 id + 描述；`Value::Object` 句柄引用，`===` 即身份比较）
    Symbol {
        /// 全局唯一 id（分配序）
        sym_id: u64,
        /// 描述文本（`Symbol()` 与 `Symbol("")` 均为空串）
        description: String,
        /// 是否**显式提供**过描述：`Symbol()` → false（description 访问器得
        /// undefined）；`Symbol("")` → true（得 ""）——规范区分两者
        has_desc: bool,
    },
    /// Map/Set 对象（键为原始 `Value` + SameValueZero 语义；`get/set/has/groupBy` 运行时）
    ///
    /// **有序存储**（`Vec` 保持插入序——`Map.prototype.entries/keys/values`、
    /// `Set.prototype.values` 迭代与 Node 一致按插入序遍历）。键保留**原始
    /// `Value`**（不再经 `to_property_key` 字符串化），查找/去重一律用
    /// SameValueZero（NaN 相等、±0 相等、对象按引用身份——见
    /// `interpreter::values_same_zero`），因此 `new Set([3, '3']).size === 2`。
    /// Set 复用之：`key` 与 `value` 同为元素原值（双槽约定，迭代从 value 槽取）。
    Map {
        /// 有序项集（键为原始 `Value`；Set 的 key = value = 元素原值）
        entries: Vec<(Value, Value)>,
    },
    /// 正则表达式对象（模式与标志原文；匹配经 `aluka-regex` 引擎求值）
    RegExp {
        /// 模式原文（不含定界符 `/`）
        pattern: String,
        /// 标志字符串（如 `i`、`g`）
        flags: String,
    },
    /// Proxy 对象（ES2015 反射代理；13 种 traps 经 handler 属性动态调用）。
    /// 目标与处理器恒为对象句柄（构造时已校验），撤销后任何操作抛 TypeError。
    Proxy {
        /// 被代理的目标对象句柄
        target: ObjectRef,
        /// 处理器对象句柄（trap 方法所在）
        handler: ObjectRef,
        /// 是否已撤销（`Proxy.revocable` 的 `revoke()` 置位）
        revoked: bool,
    },
    /// ArrayBuffer / SharedArrayBuffer 底层字节缓冲（TypedArray/DataView 共享）。
    ArrayBuffer {
        /// 字节缓冲
        data: Vec<u8>,
        /// 是否共享（SharedArrayBuffer 标记）
        shared: bool,
        /// 是否可调整大小（`resizable: true` 构造）
        resizable: bool,
        /// 可调整时的最大字节长度（不可调整为 0）
        max_byte_length: usize,
        /// 是否已分离（transfer/detach 后置位，此后全部访问抛 TypeError）
        detached: bool,
    },
    /// 类型化数组视图（11 种元素类型；视图长度以元素计，区间固定于
    /// `[byte_offset, byte_offset + length * elem_size)`）。
    TypedArray {
        /// 元素类型
        kind: crate::typed_array::TypedKind,
        /// 底层 buffer 句柄（ArrayBuffer 变体）
        buffer: ObjectRef,
        /// 起始字节偏移
        byte_offset: usize,
        /// 元素个数
        length: usize,
    },
    /// DataView 字节视图（显式字节序的整数/浮点读写）。
    DataView {
        /// 底层 buffer 句柄
        buffer: ObjectRef,
        /// 起始字节偏移
        byte_offset: usize,
        /// 视图字节长度
        byte_length: usize,
    },
    /// 原生函数（`require` 等宿主注入的可调用对象，调用由解释器拦截求值）
    NativeFn {
        /// 函数名（分派键）
        name: String,
        /// 自有属性表（如 `node:test` spy 的 `.mock` 观测面；空 = 无属性）
        properties: HashMap<String, Value>,
        /// 不可枚举属性键集合（`Object.keys` 过滤；`defineProperty`
        /// 未声明 enumerable 的静态面等）
        non_enum: HashSet<String>,
    },
    /// 可读流实例（缓冲队列 + 结束标记 + 等待中的 next promise）
    Readable {
        /// 数据缓冲队列（push 追加，next 消费）
        buffer: std::collections::VecDeque<Value>,
        /// 是否已结束（push(null) 后）
        ended: bool,
        /// 等待数据的 promise 句柄（next 空读时登记，push 时兑现）
        waiting: Option<ObjectRef>,
    },
    /// GC 清扫后的空闲占位（槽位保留、句柄稳定；分配时复用）
    Free,
}

impl Vm {
    /// 分配漏斗：安装堆对象（复用 GC 空闲槽位或追加），记账并按阈值触发回收。
    /// 全部堆分配必须经此入口（GC 触发点唯一性不变量）。
    ///
    /// **JIT 帧内不回收**：机器码的局部与操作数活在寄存器/机器栈上，当前设计
    /// 没有栈映射，GC 扫不到它们——在 JIT 帧内回收会把「只被 JIT 局部引用」的
    /// 对象误判为垃圾（`{}` 分配触发 GC 后 `o.x` 读到野槽）。因此
    /// `jit_frames > 0` 期间跳过回收；分配计数继续累积，帧退出后的第一次分配
    /// 就会补上（见 `jit_hot::jit_run` 的帧计数与 `JitCtx::frames_ptr`）。
    pub(crate) fn push_object(&mut self, obj: HeapObject) -> ObjectRef {
        let (minor_hit, major_hit) = self.gc.on_alloc();
        // JIT 帧内无栈映射、builtin 装配窗口对象登记滞后——两种窗口内
        // 一律跳过回收（计数继续累积，窗口结束后的首次分配补收）。
        if self.jit_frames == 0 && self.gc_suspended == 0 {
            if major_hit {
                self.collect_major_gc();
            } else if minor_hit {
                self.collect_minor_gc();
            } else if crate::gc::gc_stress_due(self.gc.allocated, self.gc.stress_base) {
                // 压力验证模式（ALUKA_GC_STRESS=<N>）：漏登记的根/写屏障
                // 确定性暴露为悬垂复用——全量套件在该模式下跑绿即审计闭环。
                // ALUKA_GC_MODE=major|minor 可单跑一路（诊断分代 bug 用）。
                match crate::gc::gc_stress_mode() {
                    "major" => {
                        self.collect_major_gc();
                    }
                    "minor" => {
                        self.collect_minor_gc();
                    }
                    _ => {
                        self.collect_major_gc();
                        self.collect_minor_gc();
                    }
                }
            }
        }
        if let Some(idx) = self.gc.young_free.pop().or_else(|| self.gc.old_free.pop()) {
            self.heap[idx as usize] = obj;
            self.gc.ages[idx as usize] = 0;
            self.gc.is_free[idx as usize] = false;
            self.gc.born[idx as usize] = self.gc.allocated;
            return ObjectRef(idx);
        }
        self.heap.push(obj);
        self.gc.ages.push(0);
        self.gc.is_free.push(false);
        self.gc.born.push(self.gc.allocated);
        ObjectRef((self.heap.len() - 1) as u32)
    }

    /// 在堆上分配字符串对象，返回句柄。
    pub fn alloc_string(&mut self, s: String) -> ObjectRef {
        self.push_object(HeapObject::String(s))
    }

    /// 在堆上分配 BigInt 对象（十进制字符串表示），返回句柄。
    pub fn alloc_bigint(&mut self, s: String) -> ObjectRef {
        self.push_object(HeapObject::BigInt(s))
    }

    /// 在堆上分配普通对象（带可选隐式原型），返回句柄。
    ///
    /// `proto` 为 `None` 且全局 `Object.prototype` 单例已初始化时自动挂到单例上
    /// （`{} instanceof Object` 语义）。
    pub fn alloc_ordinary_with_proto(&mut self, proto: Option<ObjectRef>) -> ObjectRef {
        self.alloc_ordinary_with_exact_proto(proto.or(self.object_prototype))
    }

    /// 在堆上分配普通对象，隐式原型精确指定（不做单例回退，
    /// 供 `Object.create(null)` 等需要无原型对象的场景）。
    pub fn alloc_ordinary_with_exact_proto(&mut self, proto: Option<ObjectRef>) -> ObjectRef {
        self.push_object(HeapObject::Ordinary {
            props: OrdinaryProps::Shape {
                shape: self.shape_table.root().id(),
                slots: Vec::new(),
            },
            getters: HashMap::new(),
            setters: HashMap::new(),
            proto,
            deleted: HashSet::new(),
            deleted_gen: 0,
            non_enum: HashSet::new(),
            has_accessors: 0,
        })
    }

    /// 在堆上分配无原型普通对象，返回句柄。
    pub fn alloc_ordinary(&mut self) -> ObjectRef {
        self.alloc_ordinary_with_proto(None)
    }

    /// 在堆上分配数组对象，返回句柄。
    ///
    /// 全局 `Array.prototype` 单例已初始化时自动挂为隐式原型
    /// （`[] instanceof Array` 语义）。
    pub fn alloc_array(&mut self, elements: Vec<Value>) -> ObjectRef {
        self.push_object(HeapObject::Array {
            elements,
            properties: HashMap::new(),
            proto: self.array_prototype,
        })
    }

    /// 在堆上分配闭包对象，返回句柄。
    ///
    /// JS 函数对象自动携带 `prototype` 属性（类机制在分配后会覆盖为自己的原型）。
    pub fn alloc_closure_with_upvalues(
        &mut self,
        func_idx: usize,
        upvalues: Vec<Upvalue>,
    ) -> ObjectRef {
        let default_proto = self.alloc_ordinary();
        let mut properties = HashMap::new();
        properties.insert("prototype".to_owned(), Value::Object(default_proto));
        let mut non_enum = HashSet::new();
        // JS 规范：函数对象 `prototype` 属性不可枚举（Object.keys 不含）
        non_enum.insert("prototype".to_owned());
        let f_ref = self.push_object(HeapObject::Closure {
            func_idx,
            upvalues,
            properties,
            getters: HashMap::new(),
            non_enum,
            proto: None,
        });
        // 规范：函数 prototype 的自有 `constructor` 回指函数自身
        //（`new F().constructor === F`——官方 assert.throws 的
        // `thrown.constructor !== ExpectedCtor` 判定依赖；缺失时实例
        // constructor 沿链落到 Object.prototype.constructor）
        let _ = self.set_property(
            Value::Object(default_proto),
            "constructor",
            Value::Object(f_ref),
        );
        f_ref
    }

    /// 在堆上分配无上值的闭包对象，返回句柄。
    pub fn alloc_closure(&mut self, func_idx: usize) -> ObjectRef {
        self.alloc_closure_with_upvalues(func_idx, Vec::new())
    }

    /// 在堆上分配原生构造器对象（自动挂 `prototype` 属性），返回句柄。
    ///
    /// `prototype` 为**不可枚举**自有属性（规范：`Object.keys(Error)` 不含它，
    /// `getOwnPropertyNames(Error)` 含）。
    pub fn alloc_native_ctor(&mut self, name: &str, prototype: Option<ObjectRef>) -> ObjectRef {
        let mut properties = HashMap::new();
        let mut non_enum = HashSet::new();
        if let Some(p) = prototype {
            properties.insert("prototype".to_owned(), Value::Object(p));
            non_enum.insert("prototype".to_owned());
        }
        self.push_object(HeapObject::NativeCtor {
            name: name.to_owned(),
            properties,
            non_enum,
        })
    }

    /// 在堆上分配原生函数对象，返回句柄。
    pub fn alloc_native_fn(&mut self, name: &str) -> ObjectRef {
        self.push_object(HeapObject::NativeFn {
            name: name.to_owned(),
            properties: HashMap::new(),
            non_enum: HashSet::new(),
        })
    }

    /// 为原生函数对象写入自有属性（如 `node:test` spy 的 `.mock` 观测面、
    /// 内建构造器的 `prototype` 表面）。
    ///
    /// `NativeFn`（`alloc_native_fn`）与 `NativeCtor`（`alloc_native_ctor`）
    /// 都带 `properties` 表：只认前者会让 `http.IncomingMessage.prototype`、
    /// `Buffer.prototype` 这类**构造器**上的写入静默丢失（真实包按
    /// `Object.create(X.prototype)` 派生原型链时会取到 undefined）。
    ///
    /// 属性值中的对象引用经 `trace_refs` 对象值分支纳入 GC 根，无需写屏障。
    pub fn set_native_fn_property(&mut self, r: ObjectRef, key: &str, val: Value) {
        match self.heap.get_mut(r.0 as usize) {
            Some(HeapObject::NativeFn { properties, .. })
            | Some(HeapObject::NativeCtor { properties, .. }) => {
                properties.insert(key.to_owned(), val);
            }
            _ => {}
        }
    }

    /// 读取原生函数对象的自有属性。
    #[must_use]
    pub fn get_native_fn_property(&self, r: ObjectRef, key: &str) -> Option<Value> {
        match self.heap.get(r.0 as usize) {
            Some(HeapObject::NativeFn { properties, .. })
            | Some(HeapObject::NativeCtor { properties, .. }) => properties.get(key).copied(),
            _ => None,
        }
    }

    /// 在堆上分配已完成（fulfilled）的 Promise 对象，返回句柄。
    pub fn alloc_fulfilled_promise(&mut self, value: Value) -> ObjectRef {
        self.push_object(HeapObject::Promise {
            pending: false,
            value,
            is_rejected: false,
            handlers: Vec::new(),
            rejected: Vec::new(),
        })
    }

    /// 在堆上分配已拒绝（rejected）的 Promise 对象，返回句柄。
    pub fn alloc_rejected_promise(&mut self, reason: Value) -> ObjectRef {
        self.push_object(HeapObject::Promise {
            pending: false,
            value: reason,
            is_rejected: true,
            handlers: Vec::new(),
            rejected: Vec::new(),
        })
    }

    /// 在堆上分配未完成（pending）的 Promise 对象，返回句柄。
    pub fn alloc_pending_promise(&mut self) -> ObjectRef {
        self.push_object(HeapObject::Promise {
            pending: true,
            value: Value::Undefined,
            is_rejected: false,
            handlers: Vec::new(),
            rejected: Vec::new(),
        })
    }

    /// 在堆上分配 Symbol 原语对象，返回句柄。
    pub fn alloc_symbol(&mut self, sym_id: u64, description: String) -> ObjectRef {
        self.alloc_symbol_described(sym_id, description, false)
    }

    /// 带"显式描述"标记的符号分配（`Symbol(desc)` 传 true）。
    pub fn alloc_symbol_described(
        &mut self,
        sym_id: u64,
        description: String,
        has_desc: bool,
    ) -> ObjectRef {
        self.push_object(HeapObject::Symbol {
            sym_id,
            description,
            has_desc,
        })
    }

    /// 在堆上分配 EventEmitter 实例，返回句柄。
    pub fn alloc_emitter(&mut self) -> ObjectRef {
        self.push_object(HeapObject::EventEmitter {
            listeners: std::collections::HashMap::new(),
        })
    }

    /// 在堆上分配可读流实例，返回句柄。
    pub fn alloc_readable(&mut self) -> ObjectRef {
        self.push_object(HeapObject::Readable {
            buffer: std::collections::VecDeque::new(),
            ended: false,
            waiting: None,
        })
    }

    /// 在堆上分配 Promise 解析器（resolve/reject 函数对象），返回句柄。
    ///
    /// 必须经 [`Self::push_object`] 漏斗：旁路 append 会让 ages/is_free 侧表
    /// 与 heap 失配（major 清扫 OOB），且绕过 GC 触发点（M6.1 压力模式实测）。
    pub fn alloc_promise_resolver(&mut self, promise: ObjectRef, resolve: bool) -> ObjectRef {
        self.push_object(HeapObject::PromiseResolver { promise, resolve })
    }

    /// 在堆上分配 Proxy 对象，返回句柄。
    pub fn alloc_proxy(&mut self, target: ObjectRef, handler: ObjectRef) -> ObjectRef {
        self.push_object(HeapObject::Proxy {
            target,
            handler,
            revoked: false,
        })
    }

    /// 在堆上分配 ArrayBuffer（`shared` 为 true 时为 SharedArrayBuffer 形态）。
    pub fn alloc_array_buffer(
        &mut self,
        data: Vec<u8>,
        shared: bool,
        resizable: bool,
        max_byte_length: usize,
    ) -> ObjectRef {
        self.push_object(HeapObject::ArrayBuffer {
            data,
            shared,
            resizable,
            max_byte_length,
            detached: false,
        })
    }

    /// 在堆上分配类型化数组视图。
    pub fn alloc_typed_array(
        &mut self,
        kind: crate::typed_array::TypedKind,
        buffer: ObjectRef,
        byte_offset: usize,
        length: usize,
    ) -> ObjectRef {
        self.push_object(HeapObject::TypedArray {
            kind,
            buffer,
            byte_offset,
            length,
        })
    }

    /// 在堆上分配 DataView 视图。
    pub fn alloc_data_view(
        &mut self,
        buffer: ObjectRef,
        byte_offset: usize,
        byte_length: usize,
    ) -> ObjectRef {
        self.push_object(HeapObject::DataView {
            buffer,
            byte_offset,
            byte_length,
        })
    }

    /// 在堆上分配 Map/Set 对象，返回句柄。
    ///
    /// `entries` 按**插入序**保留（迭代协议依赖；`set` 更新既有键时保持原位置）；
    /// 键为原始 `Value`（SameValueZero 语义，见 [`HeapObject::Map`]）。
    pub fn alloc_map(&mut self, entries: Vec<(Value, Value)>) -> ObjectRef {
        self.push_object(HeapObject::Map { entries })
    }

    /// 在堆上分配 Error 实例（`message` / `name` 为自有属性），返回句柄。
    ///
    /// 属性面按 Node 对齐（`Object.getOwnPropertyNames(new Error('m'))`）：
    /// - 传了 message（非 `undefined`）→ 自有 `message`，**不可枚举**（`Object.keys` 为空）；
    ///   未传或传 `undefined` → **无**自有 `message`（读值沿链命中 Error.prototype.message）
    ///   且 `stack` 首行无 `: ` 段（Node 实测：`new Error()` → `"Error"`）；
    /// - 自有 `name` 仅作**内部规范化**（子类构造/格式化直读自有槽），由
    ///   [`Vm::refresh_error_enumerability`] 在全部构造完成后统一标记为不可枚举，
    ///   使其在 `Object.keys`/`getOwnPropertyNames`/`JSON.stringify` 上与 Node 一致；
    /// - 自有 `stack` 字符串（首行 `Name: message`），创建后即存在（带实参调用）。
    ///
    /// `stack` 帧内容为**尽力而为**：本运行时无源映射与列号，帧取自
    /// [`crate::call::call_chain_snapshot`]（函数名 + 入口文件），行号置 0、
    /// 列号置 1，格式对齐 V8（`    at fn (file:0:1)`）；无函数帧时退化为
    /// `    at <module> (file)`。栈**结构化**：`Array.isArray(e.stack)` 为 false
    /// （Node 为字符串），`Error.captureStackTrace` 会在其后覆写为「调用点数组」形态。
    pub fn alloc_error_instance(&mut self, message: &str) -> ObjectRef {
        self.alloc_error_instance_with(message, true)
    }

    /// 构造**无自有 `message`** 的 Error 实例（`new Error()` / `new Error(undefined)`）。
    ///
    /// 规范：这两者与 `new Error('')` 的属性面不同——前者无自有 message
    /// （读值沿链得 Error.prototype.message = `''`），后者有自有 `message=''`；
    /// `stack` 首行相应为 `Error`（无 `: ` 段），与 Node 实测一致。
    pub fn alloc_error_instance_no_message(&mut self) -> ObjectRef {
        self.alloc_error_instance_with("", false)
    }

    /// 构造实现：`has_message` 决定是否落自有 `message` 槽。
    fn alloc_error_instance_with(&mut self, message: &str, has_message: bool) -> ObjectRef {
        let err_proto = self.error_prototype.or(self.object_prototype);
        let obj = self.alloc_ordinary_with_exact_proto(err_proto);
        // **键序**：Node 的 `Object.getOwnPropertyNames(new Error('m'))` 为
        // `["stack", "message"]`——先落 stack（创建时刻捕获调用帧），再落 message。
        let stack_text = self.build_error_stack(message, has_message);
        let stack_ref = self.alloc_string(stack_text);
        let _ = self.set_property(Value::Object(obj), "stack", Value::Object(stack_ref));
        // `name` **不**落实例（规范：`Error.prototype.name`；`e.name` 沿链命中）。
        // 子类实例的 name 由 `attach_error_proto` 挂到各自原型上。
        if has_message {
            let message_ref = self.alloc_string(message.to_owned());
            let _ = self.set_property(Value::Object(obj), "message", Value::Object(message_ref));
        }
        obj
    }

    /// 生成错误 `stack` 文本（首行 `Name: message`，其后为调用帧）。
    fn build_error_stack(&mut self, message: &str, has_message: bool) -> String {
        self.build_error_stack_with(message, has_message, None)
    }

    /// `stack` 生成（可指定 **constructorOpt**）。
    ///
    /// `constructor_opt` 非空时（`Error.captureStackTrace(target, ctor)`）：
    /// 从最内层向外扫描，**丢弃该构造器帧及其内侧帧**（Node 语义：
    /// 「all frames above constructorOpt, including constructorOpt, will be omitted」），
    /// 只保留其调用者。
    ///
    /// 帧数按 `Error.stackTraceLimit`（用户可写，默认 10）截断——`0` 表示
    /// 只保留首行，与 Node 实测一致。
    pub(crate) fn build_error_stack_with(
        &mut self,
        message: &str,
        has_message: bool,
        constructor_opt: Option<ObjectRef>,
    ) -> String {
        // 名字取**当前有效名**（沿原型链读 `name`）：子类构造在 alloc 之后
        // 才改原型，故这里通常得到 "Error"，随后由 `refresh_error_stack_name`
        // 按最终 name 同步首行（`TypeError: msg`）。
        let name = self
            .error_prototype
            .and_then(|p| self.own_value(p.index(), "name"))
            .map(|v| self.format_value(v))
            .unwrap_or_else(|| "Error".to_owned());
        let head = if has_message && !message.is_empty() {
            format!("{name}: {message}")
        } else {
            name
        };
        let file = if self.entry_file.is_empty() {
            "<anonymous>".to_owned()
        } else {
            self.entry_file.clone()
        };
        // 调用链顺序为 [最外层 … 最内层]，V8 打印顺序相反（最内层在前）。
        let frames = crate::call::call_chain_snapshot();
        let skip_idx = constructor_opt.and_then(|c| {
            let target = match self.heap.get(c.index()) {
                Some(HeapObject::Closure { func_idx, .. }) => *func_idx,
                _ => return None,
            };
            frames.iter().rposition(|(f, _)| *f == target)
        });
        let limit = self.error_stack_trace_limit();
        let mut out = head;
        let mut emitted = 0usize;
        for (i, (_, fname)) in frames.iter().enumerate().rev() {
            // constructorOpt：丢弃该帧及其**内侧**帧（索引更大者）
            if skip_idx.is_some_and(|s| i >= s) {
                continue;
            }
            if emitted >= limit {
                break;
            }
            if fname.is_empty() {
                out.push_str(&format!("\n    at <anonymous> ({file}:0:1)"));
            } else {
                out.push_str(&format!("\n    at {fname} ({file}:0:1)"));
            }
            emitted += 1;
        }
        if emitted == 0 && frames.is_empty() {
            // 无任何解释帧：模块顶层退化为单帧（保持既有形态）
            if limit > 0 {
                out.push_str(&format!("\n    at <module> ({file})"));
            }
        }
        out
    }

    /// `Error.stackTraceLimit` 的当前有效值（用户可写；非数值/未设置回退 10）。
    ///
    /// Node 语义：控制 `stack` 的**帧数上限**（`0` → 只留首行）。
    fn error_stack_trace_limit(&mut self) -> usize {
        const DEFAULT_LIMIT: usize = 10;
        let Some(ctor) = self.error_ctor else {
            return DEFAULT_LIMIT;
        };
        match self.own_value(ctor.index(), "stackTraceLimit") {
            Some(v) => match v.case() {
                ValueCase::Number(n) if n.is_finite() && n >= 0.0 => n as usize,
                _ => DEFAULT_LIMIT,
            },
            None => DEFAULT_LIMIT,
        }
    }

    /// 刷新错误实例的属性面（**构造收尾统一入口**）：
    /// 把自有 `name`/`message`/`stack` 标记为不可枚举（Node：`Object.keys(err)` 为空集），
    /// 并在 name 已改为子类名时同步 `stack` 首行。
    ///
    /// 调用时机：`new Error/TypeError/...` 构造分支、`attach_error_proto`、
    /// `typed_error` 返回前——这些点之后 name/stack 不再变化。
    pub(crate) fn refresh_error_enumerability(&mut self, err: ObjectRef) {
        for key in ["name", "message", "stack"] {
            self.mark_non_enumerable(Value::Object(err), key);
        }
    }

    /// 依据实例当前**有效** `name`（沿原型链，实例通常无自有 name）同步
    /// `stack` 首行（子类构造 / `attach_error_proto` 后调用）。
    ///
    /// 只重写首行、保留既有帧；`stack` 缺失/非字符串时按当前 name+message
    /// **重新生成**（不能写回 `"undefined"`——那会让 `String(err.stack)` 得到
    /// 字面量 `"undefined"`，实测缺陷）。
    pub(crate) fn refresh_error_stack_name(&mut self, err: ObjectRef) {
        let name = self
            .get_property(Value::Object(err), "name")
            .ok()
            .map(|v| self.format_value(v))
            .filter(|s| !s.is_empty() && s != "undefined")
            .unwrap_or_else(|| "Error".to_owned());
        let message = self
            .get_property(Value::Object(err), "message")
            .ok()
            .map(|v| self.format_value(v))
            .filter(|s| s != "undefined")
            .unwrap_or_default();
        let head = if message.is_empty() {
            name
        } else {
            format!("{name}: {message}")
        };
        // 既有 stack 的帧部分（第二行起），仅当是非空字符串时沿用
        let frames = self
            .own_value(err.index(), "stack")
            .and_then(|v| v.as_object())
            .and_then(|r| match self.heap.get(r.index()) {
                Some(HeapObject::String(s)) => Some(s.clone()),
                _ => None,
            })
            .and_then(|s| s.split_once('\n').map(|(_, rest)| rest.to_owned()));
        let new_stack = match frames {
            Some(rest) if !rest.is_empty() => format!("{head}\n{rest}"),
            _ => head,
        };
        let v = self.alloc_string(new_stack);
        let _ = self.set_property(Value::Object(err), "stack", Value::Object(v));
    }

    /// 重新生成实例的 `stack`（`Error.captureStackTrace` 用）：
    /// `target.stack` 覆盖为按 `Name: message` + 当前调用链生成的**字符串**
    /// （Node 形态；不再写「调用点数组」）。
    pub(crate) fn fill_error_stack(
        &mut self,
        target: ObjectRef,
        constructor_opt: Option<ObjectRef>,
    ) {
        // message：优先自有槽，其次沿原型链（空串视为无 message）
        let message = self
            .own_value(target.index(), "message")
            .or_else(|| {
                self.get_property(Value::Object(target), "message")
                    .ok()
                    .filter(|v| !v.is_undefined())
            })
            .map(|v| self.format_value(v))
            .filter(|s| !s.is_empty() && s != "undefined")
            .unwrap_or_default();
        let has_message = !message.is_empty();
        // Node 语义：Error.prepareStackTrace 为函数时 stack = 其返回值
        if let Some(prepared) = self.prepare_error_stack(Value::Object(target), constructor_opt) {
            let _ = self.set_property(Value::Object(target), "stack", prepared);
            self.refresh_error_enumerability(target);
            return;
        }
        let text = self.build_error_stack_with(&message, has_message, constructor_opt);
        let v = self.alloc_string(text);
        let _ = self.set_property(Value::Object(target), "stack", Value::Object(v));
        self.refresh_error_enumerability(target);
    }

    /// 构造 callsite 对象数组（`Error.prepareStackTrace` 的第二个实参）。
    ///
    /// 每个元素提供真实包（`depd` / Express 依赖链）所需的方法面：`getFileName`
    /// /`getLineNumber`/`getColumnNumber`/`getFunctionName`/`getTypeName`/`getThis`
    /// /`getEvalOrigin`/`isEval`/`isNative`/`isConstructor`/`isToplevel`/`toString`。
    /// 本运行时无源映射与列号：行号 0、列号 1，文件名取入口文件。
    pub(crate) fn build_callsite_array(
        &mut self,
        constructor_opt: Option<ObjectRef>,
    ) -> Vec<Value> {
        let file = if self.entry_file.is_empty() {
            "<anonymous>".to_owned()
        } else {
            self.entry_file.clone()
        };
        let frames = crate::call::call_chain_snapshot();
        let skip_idx = constructor_opt.and_then(|c| {
            let target = match self.heap.get(c.index()) {
                Some(HeapObject::Closure { func_idx, .. }) => *func_idx,
                _ => return None,
            };
            frames.iter().rposition(|(f, _)| *f == target)
        });
        let limit = self.error_stack_trace_limit();
        let mut out: Vec<Value> = Vec::new();
        for (i, (_, fname)) in frames.iter().enumerate().rev() {
            if skip_idx.is_some_and(|s| i >= s) {
                continue;
            }
            if out.len() >= limit {
                break;
            }
            let site = self.alloc_ordinary();
            let ns = self.alloc_string("callsite".to_owned());
            let _ = self.set_property(Value::Object(site), "_builtinNs", Value::Object(ns));
            let file_v = Value::Object(self.alloc_string(file.clone()));
            let _ = self.set_property(Value::Object(site), "_file", file_v);
            let name_v = Value::Object(self.alloc_string(fname.to_string()));
            let _ = self.set_property(Value::Object(site), "_funcName", name_v);
            out.push(Value::Object(site));
        }
        out
    }

    /// 调用 `Error.prepareStackTrace(err, callSites)` 并返回其结果
    /// （钩子缺失或不可调用时返回 `None`，调用方回退字符串 stack）。
    ///
    /// 真实包用法（`depd/index.js::getStack`）：
    /// ```js
    /// Error.prepareStackTrace = prepareObjectStackTrace;
    /// Error.captureStackTrace(obj);
    /// var stack = obj.stack.slice(1);   // 期望数组
    /// ```
    fn prepare_error_stack(
        &mut self,
        err: Value,
        constructor_opt: Option<ObjectRef>,
    ) -> Option<Value> {
        let ctor = self.error_ctor?;
        let hook = self.own_value(ctor.index(), "prepareStackTrace")?;
        if hook.is_undefined() {
            return None;
        }
        let (fi, uvs) = self.resolve_callable(hook);
        let fi = fi?;
        let sites = self.build_callsite_array(constructor_opt);
        let arr = Value::Object(self.alloc_array(sites));
        self.invoke_function(fi, Value::Undefined, &[err, arr], uvs)
            .ok()
    }
}

impl HeapObject {
    /// 遍历对象持有的全部堆引用（GC 标记用；叶子对象为空集）。
    pub fn trace_refs(&self, mut f: impl FnMut(u32)) {
        match self {
            HeapObject::Ordinary {
                props,
                getters,
                setters,
                proto,
                ..
            } => {
                match props {
                    OrdinaryProps::Shape { slots, .. } => {
                        // 槽位为 NaN-box：对象引用以盒形式存在
                        for &b in slots {
                            if aluka_jit::valbox::is_object(b) {
                                f(aluka_jit::valbox::unbox_object(b));
                            }
                        }
                    }
                    OrdinaryProps::Dict { properties, .. } => {
                        for (_, v) in properties {
                            if let Some(r) = v.as_object() {
                                f(r.0);
                            }
                        }
                    }
                }
                for v in getters.values().chain(setters.values()) {
                    if let Some(r) = v.as_object() {
                        f(r.0);
                    }
                }
                if let Some(p) = proto {
                    f(p.0);
                }
            }
            HeapObject::Array {
                elements,
                properties,
                proto,
            } => {
                for v in properties.values() {
                    if let Some(r) = v.as_object() {
                        f(r.0);
                    }
                }
                for v in elements {
                    if let Some(r) = v.as_object() {
                        f(r.0);
                    }
                }
                if let Some(p) = proto {
                    f(p.0);
                }
            }
            HeapObject::Closure {
                upvalues,
                properties,
                getters,
                proto,
                ..
            } => {
                for uv in upvalues {
                    if let Some(r) = uv.0.borrow().as_object() {
                        f(r.0);
                    }
                }
                for v in properties.values().chain(getters.values()) {
                    if let Some(r) = v.as_object() {
                        f(r.0);
                    }
                }
                if let Some(p) = proto {
                    f(p.0);
                }
            }
            HeapObject::NativeCtor { properties, .. } => {
                for v in properties.values() {
                    if let Some(r) = v.as_object() {
                        f(r.0);
                    }
                }
            }
            HeapObject::Promise {
                value,
                handlers,
                rejected,
                ..
            } => {
                if let Some(r) = value.as_object() {
                    f(r.0);
                }
                for h in handlers.iter().chain(rejected.iter()) {
                    if let Some(r) = h.as_object() {
                        f(r.0);
                    }
                }
            }
            HeapObject::PromiseResolver { promise, .. } => f(promise.0),
            HeapObject::EventEmitter { listeners } => {
                for entries in listeners.values() {
                    for (v, _) in entries {
                        if let Some(r) = v.as_object() {
                            f(r.0);
                        }
                    }
                }
            }
            HeapObject::Map { entries } => {
                // 键与值都是原始 `Value`（键不再字符串化），两者都可能引用
                // 堆对象：必须全部标记——漏标键会让 GC 误回收键对象（悬垂）
                for (k, v) in entries {
                    if let Some(r) = k.as_object() {
                        f(r.0);
                    }
                    if let Some(r) = v.as_object() {
                        f(r.0);
                    }
                }
            }
            HeapObject::Readable {
                buffer, waiting, ..
            } => {
                for v in buffer {
                    if let Some(r) = v.as_object() {
                        f(r.0);
                    }
                }
                if let Some(w) = waiting {
                    f(w.0);
                }
            }
            HeapObject::NativeFn { properties, .. } => {
                for v in properties.values() {
                    if let Some(r) = v.as_object() {
                        f(r.0);
                    }
                }
            }
            HeapObject::Proxy {
                target, handler, ..
            } => {
                f(target.0);
                f(handler.0);
            }
            HeapObject::TypedArray { buffer, .. } | HeapObject::DataView { buffer, .. } => {
                f(buffer.0);
            }
            // ArrayBuffer 为字节缓冲叶子对象；其余叶子对象：无堆引用
            HeapObject::ArrayBuffer { .. } => {}
            HeapObject::String(_)
            | HeapObject::BigInt(_)
            | HeapObject::Symbol { .. }
            | HeapObject::RegExp { .. }
            | HeapObject::Generator
            | HeapObject::Free => {}
        }
    }
}
