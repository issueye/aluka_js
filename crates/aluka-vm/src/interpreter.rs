//! 虚拟机核心解释器：执行状态定义与操作码分派循环。

use crate::exception::{Completion, FinallyOutcome, PHASE_TRY, TryExitOutcome, TryHandler};
use crate::generator::GeneratorState;
use crate::heap::HeapObject;
use crate::ops::{eq, js_number_to_string, parse_js_number, strict_eq, to_boolean, to_number};
use crate::value::{Upvalue, Value, ValueCase};
use aluka_bytecode::{ClassTemplate, Constant, FuncTemplate, Instr, Op, TryEntry};
use aluka_core::{ObjectRef, ShapeTable};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;

/// 执行期可能发生的错误。
#[derive(Debug, Clone, PartialEq)]
pub enum VmError {
    /// 操作数栈下溢（Pop 时栈为空）
    StackUnderflow,
    /// 访问越界局部变量槽位
    LocalOutOfRange,
    /// 函数执行到达末尾但未返回
    MissingReturn,
    /// 整数除以零
    DivisionByZero,
    /// JS 层抛出的异常值（`THROW` 或未捕获时沿调用链传播）
    Thrown(Value),
    /// 生成器 `YIELD` 挂起信号（携带产出的值，由生成器驱动层捕获）
    Yielded(Value),
    /// `AWAIT` 未完成 Promise 的挂起信号（携带 promise 句柄，由 async 驱动层捕获）
    Awaited(aluka_core::ObjectRef),
    /// `process.exit(code)` 终止信号（Node 语义立即终止事件循环；
    /// 不参与 try/catch 匹配，沿调用链直达宿主）
    Exit(i32),
    /// 遇到了当前里程碑尚未实现的操作码
    UnimplementedOpcode(Op),
}

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StackUnderflow => write!(f, "操作数栈下溢"),
            Self::LocalOutOfRange => write!(f, "访问的局部变量槽位越界"),
            Self::MissingReturn => write!(f, "指令流在返回前结束"),
            Self::DivisionByZero => write!(f, "除以零错误"),
            Self::Thrown(_) => write!(f, "未捕获的 JS 异常"),
            Self::Yielded(_) => write!(f, "生成器挂起信号（不应逃逸到顶层）"),
            Self::Awaited(_) => write!(f, "async 挂起信号（不应逃逸到顶层）"),
            Self::Exit(code) => write!(f, "process.exit({code})"),
            Self::UnimplementedOpcode(op) => write!(f, "未实现的操作码: {op:?}"),
        }
    }
}

impl std::error::Error for VmError {}

/// 一次执行的状态：操作数栈与局部槽位。
#[derive(Default)]
pub struct Vm {
    /// 操作数栈
    pub stack: Vec<Value>,
    /// 局部变量槽位
    pub locals: Vec<Value>,
    /// 控制台打印捕获记录（供测试断言比对）
    pub stdout_records: Vec<String>,
    /// 当前执行函数的常量池（Rc 共享，帧切换零拷贝）
    pub current_constants: std::rc::Rc<Vec<Constant>>,
    /// 当前模块的常量池表（与 `module_functions` 平行，供 `invoke_function` 零拷贝换帧）
    pub(crate) module_constants: Vec<std::rc::Rc<Vec<Constant>>>,
    /// 当前模块的函数扩展标量头（arguments 槽位等；与函数表平行）
    pub(crate) module_header_extras: Vec<aluka_bytecode::FuncHeaderExtras>,
    /// 堆对象存储
    pub heap: Vec<HeapObject>,
    /// 隐藏类表（Ordinary 对象的属性名 → 槽位映射；程序生命周期内不随模块重置）
    pub(crate) shape_table: ShapeTable,
    /// per-函数 JIT 状态（与 `module_functions` 平行；`run_module` 重置）
    pub(crate) jit_slots: Vec<crate::jit_hot::JitSlot>,
    /// per-函数调用计数（达 `JIT_HOT_THRESHOLD` 触发编译）
    pub(crate) jit_counters: Vec<u32>,
    /// JIT 代数：`jit_reset`（模块函数表替换）时取新值。
    ///
    /// 已发射机器码内的调用内联缓存记着上一代的裸入口地址；编译产物随
    /// `jit_slots` 释放后地址失效，代数守卫是唯一拦截手段（见 `jit_hot`）。
    pub(crate) jit_gen: u32,
    /// JIT `CALL` 落到 helper 的次数（调用 IC 未命中）。
    pub(crate) jit_call_fallbacks: u64,
    /// 是否允许函数入口尝试 Tier 1 JIT。
    ///
    /// 默认开启以保持既有运行时行为；解释器基准和 Tier 0 回归可显式关闭，
    /// 从而不把热点编译/机器码执行混入解释器数字。
    pub(crate) jit_enabled: bool,
    /// 活跃 JIT 帧数（机器码在入口自增、返回点自减）。
    ///
    /// JIT 的局部变量与操作数活在寄存器/机器栈上，**没有栈映射**，GC 无从
    /// 把它们登记为根。因此计数 > 0 期间禁止回收（见 `push_object`）——
    /// 这是当前无栈映射设计下 GC 安全的唯一保证；地址随 `Vm` 稳定，经
    /// `JitCtx::frames_ptr` 暴露给机器码。
    pub(crate) jit_frames: u32,
    /// 复用的 JIT 上下文（装箱以保证地址稳定）。
    ///
    /// vtable/layout 在进程内恒定，只有 `vm`/`consts_*`/`heap_ptr` 随调用变化；
    /// 每次调用重建整个 `JitCtx`（约 160 字节）在「每轮都进 JIT」的负载上
    /// 是可测开销，故复用 + 三字段保存恢复（嵌套 JIT 调用安全）。
    pub(crate) jit_ctx: Option<Box<aluka_jit::ctx::JitCtx>>,
    /// 模块全部函数模板（供跨函数调用与 Getter 调度；Rc 共享避免逐调用深拷贝）
    pub module_functions: Vec<std::rc::Rc<FuncTemplate>>,
    /// 模块全部类模板（供 MakeClass 构造类对象与原型链）
    pub module_classes: Vec<ClassTemplate>,
    /// 当前函数帧所持有的上值列表（供 LoadUpvalue / StoreUpvalue 访问）
    pub current_upvalues: Vec<Upvalue>,
    /// 当前函数帧活跃的打开上值表（slot -> Upvalue），保证同一 slot 共享同一个 RefCell
    pub open_upvalues: HashMap<usize, Upvalue>,
    /// 当前函数帧活跃的 try/catch/finally handler 栈（自底向内层递增）
    pub(crate) try_stack: Vec<TryHandler>,
    /// 当前函数模板的 Try 表（`TRY_ENTER` 按索引克隆条目入栈）
    pub(crate) current_try_table: Vec<TryEntry>,
    /// 全局变量表（`STORE_GLOBAL` 写入、`LOAD_GLOBAL` 优先读取）
    pub globals: HashMap<String, Value>,
    /// `Object.prototype` 单例（普通对象默认隐式原型）
    pub object_prototype: Option<ObjectRef>,
    /// `Array.prototype` 单例（数组默认隐式原型）
    pub array_prototype: Option<ObjectRef>,
    /// `Math` 内置对象单例
    pub math_object: Option<ObjectRef>,
    /// `Error` 原生构造器单例
    pub error_ctor: Option<ObjectRef>,
    /// `Error.prototype` 独立单例（链 `object_prototype`；曾与 Object.prototype
    /// 共享同一对象导致任意普通对象 `instanceof Error` 误判 true——http-errors
    /// createError 的 props 对象被当作 Error 的根因）
    pub error_prototype: Option<ObjectRef>,
    /// `Array` 原生构造器单例
    pub array_ctor: Option<ObjectRef>,
    /// `Object` 原生构造器单例
    pub object_ctor: Option<ObjectRef>,
    /// `RegExp` 原生构造器单例（字面量与 `new RegExp` 共用表面）
    pub regexp_ctor: Option<ObjectRef>,
    /// `RegExp.prototype`（`constructor` 回指构造器）
    pub(crate) regexp_prototype: Option<ObjectRef>,
    /// `Object.prototype.hasOwnProperty` 原生函数单例。不挂到原型对象
    /// 上（for-in 会枚举到它，与 Node.js 22 LTS 标准 不一致）；属性链查不到时
    /// 在 [`Vm::get_property`] 末尾合成返回。
    pub(crate) objproto_has_own: Option<ObjectRef>,
    /// `fs` 内置对象单例（readFileSync/writeFileSync 拦截）
    pub fs_object: Option<ObjectRef>,
    /// `require` 原生函数句柄（`setup_cjs` 后可用）
    pub require_fn: Option<ObjectRef>,
    /// 模块专属 require 函数实例 → 其模块目录（`require('./x')` 相对闭包
    /// 所属模块解析；Node 语义：require 为模块闭包捕获，延迟调用仍解析
    /// 到模块自身目录，而非当前加载栈顶）
    pub(crate) require_bases: HashMap<ObjectRef, PathBuf>,
    /// CJS 模块缓存：规范化路径 → exports
    pub(crate) module_exports: HashMap<String, Value>,
    /// 模块解析基准目录（入口文件所在目录）
    pub(crate) base_dir: Option<std::path::PathBuf>,
    /// require 基准目录栈：栈顶 = 当前模块所在目录（嵌套 require 相对
    /// 自身解析；空栈回退 base_dir）
    pub(crate) require_base_stack: Vec<std::path::PathBuf>,
    /// 入口文件路径（CJS `__filename`）
    pub(crate) entry_file: String,
    /// 最近执行指令的下标（错误定位用）
    pub last_pc: usize,
    /// 解释器属性读取 IC（直接映射，见 `pic.rs`）
    pub(crate) prop_ic: Vec<crate::pic::PropIcEntry>,
    /// IC 命中计数（诊断/测试观测面）
    pub pic_hits: u64,
    /// 方法调用 IC（直接原型绑定，见 `pic.rs`）
    pub(crate) method_ic: Vec<crate::pic::MethodIcEntry>,
    /// 当前执行函数索引（错误定位用；-1 表示无）
    pub current_func_idx: i64,
    /// LCOV 行覆盖计数（`aluka test --test-reporter=lcov` 才挂载；默认 None
    /// ——主循环每指令一次 Option 判定，关闭态近零成本）
    pub coverage: Option<crate::coverage::Coverage>,
    /// nextTick 优先微任务队列（回调函数）
    pub(crate) nexttick_queue: std::collections::VecDeque<Value>,
    /// Promise 微任务队列（Job：回调或帧恢复）
    pub(crate) microtask_queue: std::collections::VecDeque<crate::builtins::Job>,
    /// 宏任务队列（句柄 id + 到期累计毫秒 + 延迟 + 回调 + 是否周期）
    pub(crate) macro_tasks: std::collections::VecDeque<(u64, u64, u64, Value, bool)>,
    /// 真实 worker 线程 spawn 钩子（装配层注入；None 时 `new Worker` 走
    /// 同进程伪 worker 路径，见 `worker_threads` 模块文档）
    pub worker_entry: Option<std::sync::Arc<crate::worker::WorkerEntryFn>>,
    /// 定时器句柄计数器（setTimeout/setInterval 分配 id）
    pub(crate) timer_counter: u64,
    /// 已被 clear 的定时器句柄集合（drain 时跳过）
    pub(crate) active_timers: std::collections::HashSet<u64>,
    /// `Promise` 原生构造器单例（resolve/withResolvers 拦截）
    pub promise_ctor: Option<ObjectRef>,
    /// `Map` 原生构造器单例（groupBy 拦截）
    pub map_ctor: Option<ObjectRef>,
    /// `Set` 原生构造器单例
    pub set_ctor: Option<ObjectRef>,
    /// `Proxy` 原生构造器单例（revocable/isProxy 静态面挂接于其上）
    pub proxy_ctor: Option<ObjectRef>,
    /// `Reflect` 全局对象单例（延迟物化）
    pub reflect_object: Option<ObjectRef>,
    /// 运行时编译器 Hook（eval / new Function 动态求值；宿主经
    /// `set_eval_provider` 装配，后端仅接收字节码，保持 ISA 解耦）
    pub(crate) eval_provider: Option<crate::eval::EvalProvider>,
    /// 最近一次模块入口异步完成时的未完成 Promise（`__aluka_import__`
    /// 依赖完成链用；M2.2）
    pub(crate) last_entry_async_promise: Option<Value>,
    /// GC 钉扎句柄（require 进行中的 module 对象；防嵌套加载期间被回收）
    pub(crate) gc_pinned: Vec<u32>,
    /// GC 挂起计数（builtin 装配窗口；>0 时 push_object 跳过回收）
    pub(crate) gc_suspended: u32,
    /// 保存帧寄存器：嵌套执行期间被换出的外层帧状态（GC 根集合成员，
    /// 见 [`crate::gc::SavedFrameState`]；漏登记 = 换出帧悬垂复用）
    pub(crate) gc_saved_frames: Vec<crate::gc::SavedFrameState>,
    /// CJS 模块作用域表（与函数表 append 平行；注入名按模块隔离，
    /// 见 [`crate::modules::ModuleScopeRecord`]）
    pub(crate) module_scopes: Vec<crate::modules::ModuleScopeRecord>,
    /// `Function` 全局构造器单例（M2.4：函数对象面补全——prototype 链）
    pub function_ctor: Option<ObjectRef>,
    /// 原型方法面单例（surface 模块挂载；`X.prototype` 对象与方法属性）
    pub str_proto: Option<ObjectRef>,
    /// `Boolean.prototype` 原型面单例
    pub bool_proto: Option<ObjectRef>,
    /// `Number.prototype` 原型面单例
    pub num_proto: Option<ObjectRef>,
    /// `Function.prototype` 原型面单例
    pub fn_proto: Option<ObjectRef>,
    /// `RegExp.prototype` 原型面单例
    pub regexp_proto: Option<ObjectRef>,
    /// `Array.prototype` 原型面单例
    pub array_proto_surface: Option<ObjectRef>,
    /// Set/Map/WeakSet/WeakMap/WeakRef 共享原型面单例
    pub container_proto: Option<ObjectRef>,
    /// `Symbol.prototype` 原型面单例
    pub symbol_proto: Option<ObjectRef>,
    /// `Date.prototype` 原型面单例（实例方法面挂载点；实例 `[[Prototype]]` 指向它）
    pub date_proto: Option<ObjectRef>,
    /// 原型面构造器单例缓存（String/Boolean/Number/Set/Map/... 名 → NativeCtor）
    pub ctor_cache: std::collections::HashMap<String, ObjectRef>,
    /// `process` 全局对象单例（nextTick 拦截）
    pub process_object: Option<ObjectRef>,
    /// `process.env` 对象单例缓存（物化一次；键大小写不敏感语义见 property.rs）
    pub(crate) env_object: Option<ObjectRef>,
    /// `path` 内置模块单例（join/basename/dirname/extname/resolve 拦截）
    pub path_module: Option<ObjectRef>,
    /// `os` 内置模块单例（platform/homedir/tmpdir 拦截；EOL 属性读取特判）
    pub os_module: Option<ObjectRef>,
    /// `stream` 内置模块单例（Readable 构造器属性物化）
    pub stream_module: Option<ObjectRef>,
    /// `events` 内置模块单例（EventEmitter 构造器属性物化）
    pub events_module: Option<ObjectRef>,
    /// 内置库注册表（querystring/constants 等并行开发模块的分派表）
    pub(crate) builtin_registry: crate::builtins::BuiltinRegistry,
    /// 内置库活跃事件源表（net/http/child_process 等 I/O 泵；随 Vm 生命周期）
    pub(crate) event_sources: Vec<(&'static str, crate::builtins::EventSourcePump)>,
    /// GC 侧表（年龄/free-list/统计；ADR 0002 分代标记-清除）
    pub(crate) gc: crate::gc::GcState,
    /// 挂起 async 帧的恢复登记：promise 句柄索引 → 恢复帧
    pub(crate) promise_resumes: HashMap<u32, crate::builtins::PendingResume>,
    /// 生成器对象注册表（堆句柄索引 → 执行状态）
    pub(crate) generators: HashMap<u32, GeneratorState>,
    /// 最近一次 `YIELD` 的恢复点（下一条指令索引）
    pub(crate) yield_pc: usize,
}

impl Vm {
    /// 公开全局表的内容 epoch（供 JIT Global IC 守卫）。
    ///
    /// `globals` 保持公开以兼容现有宿主 API，无法审计所有外部 `insert/remove`；
    /// 因此不依赖写屏障，而在每次 JIT 入口计算稳定摘要。只用于缓存失效，不是
    /// 加密哈希；碰撞只会导致旧值被保留，故使用长度、键和值的完整机器字组合。
    pub(crate) fn globals_epoch(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        use std::hash::{Hash, Hasher};
        self.globals.len().hash(&mut h);
        for (k, v) in &self.globals {
            k.hash(&mut h);
            match v.case() {
                ValueCase::Undefined => 0u8.hash(&mut h),
                ValueCase::Null => 1u8.hash(&mut h),
                ValueCase::Boolean(b) => {
                    2u8.hash(&mut h);
                    b.hash(&mut h);
                }
                ValueCase::Number(n) => {
                    3u8.hash(&mut h);
                    n.to_bits().hash(&mut h);
                }
                ValueCase::Object(r) => {
                    4u8.hash(&mut h);
                    r.0.hash(&mut h);
                }
            }
        }
        h.finish()
    }

    /// `globals_epoch` 的低 32 位（JIT ctx 使用机器字宽字段）。
    pub(crate) fn globals_epoch32(&self) -> u32 {
        self.globals_epoch() as u32
    }

    /// 关闭或开启 Tier 1 JIT。返回旧状态，便于基准临时切换并恢复。
    pub fn set_jit_enabled(&mut self, enabled: bool) -> bool {
        let old = self.jit_enabled;
        self.jit_enabled = enabled;
        old
    }

    /// 当前是否开启 Tier 1 JIT。
    #[must_use]
    pub fn jit_enabled(&self) -> bool {
        self.jit_enabled
    }

    ///
    /// 同时在堆上预建内置原型与构造器单例（`Object.prototype`、`Array.prototype`、
    /// `Math`、`Error`/`Array`/`Object` 构造器），后续所有分配自动挂接正确的原型链。
    #[must_use]
    pub fn new(locals: usize) -> Self {
        let mut vm = Self {
            stack: Vec::new(),
            locals: vec![Value::Undefined; locals],
            stdout_records: Vec::new(),
            current_constants: std::rc::Rc::new(Vec::new()),
            module_constants: Vec::new(),
            module_header_extras: Vec::new(),
            heap: Vec::new(),
            shape_table: ShapeTable::new(),
            jit_slots: Vec::new(),
            jit_counters: Vec::new(),
            jit_gen: crate::jit_hot::next_jit_gen(),
            jit_call_fallbacks: 0,
            jit_frames: 0,
            jit_enabled: true,
            jit_ctx: None,
            module_functions: Vec::new(),
            module_classes: Vec::new(),
            current_upvalues: Vec::new(),
            open_upvalues: HashMap::new(),
            try_stack: Vec::new(),
            current_try_table: Vec::new(),
            globals: HashMap::new(),
            object_prototype: None,
            array_prototype: None,
            math_object: None,
            error_ctor: None,
            error_prototype: None,
            array_ctor: None,
            object_ctor: None,
            regexp_ctor: None,
            regexp_prototype: None,
            objproto_has_own: None,
            generators: HashMap::new(),
            yield_pc: 0,
            last_pc: 0,
            prop_ic: crate::pic::pic_table_new(),
            pic_hits: 0,
            method_ic: crate::pic::method_ic_table_new(),
            current_func_idx: -1,
            coverage: None,
            nexttick_queue: std::collections::VecDeque::new(),
            microtask_queue: std::collections::VecDeque::new(),
            macro_tasks: std::collections::VecDeque::new(),
            timer_counter: 0,
            active_timers: std::collections::HashSet::new(),
            promise_ctor: None,
            map_ctor: None,
            set_ctor: None,
            proxy_ctor: None,
            reflect_object: None,
            eval_provider: None,
            last_entry_async_promise: None,
            gc_pinned: Vec::new(),
            gc_suspended: 0,
            gc_saved_frames: Vec::new(),
            module_scopes: Vec::new(),
            function_ctor: None,
            str_proto: None,
            bool_proto: None,
            num_proto: None,
            fn_proto: None,
            regexp_proto: None,
            array_proto_surface: None,
            container_proto: None,
            symbol_proto: None,
            date_proto: None,
            ctor_cache: std::collections::HashMap::new(),
            process_object: None,
            env_object: None,
            path_module: None,
            os_module: None,
            stream_module: None,
            events_module: None,
            builtin_registry: std::default::Default::default(),
            event_sources: Vec::new(),
            worker_entry: None,
            gc: crate::gc::GcState::default(),
            promise_resumes: HashMap::new(),
            module_exports: HashMap::new(),
            base_dir: None,
            require_base_stack: Vec::new(),
            entry_file: String::new(),
            require_fn: None,
            require_bases: HashMap::new(),
            fs_object: None,
        };
        // Object.prototype：原型链顶端（[[Prototype]] 为 null）。保持
        // 零自有属性（for-in 口径与 Node.js 22 LTS 标准 一致）；hasOwnProperty 以
        // 单例合成，见 get_property 末尾
        vm.object_prototype = Some(vm.alloc_ordinary());
        vm.objproto_has_own = Some(vm.alloc_native_fn("objproto.hasOwnProperty"));
        // Array.prototype：沿原型链挂到 Object.prototype，并填充已实现的
        // 数组方法（NativeFn 占位：CALL_METHOD 按名字分派求值，属性存在性
        // 查询（`arr.forEach ? ...`）经原型链命中）
        vm.array_prototype = vm
            .object_prototype
            .map(|p| vm.alloc_ordinary_with_proto(Some(p)));
        if let Some(ap) = vm.array_prototype {
            let methods = [
                "map",
                "filter",
                "find",
                "some",
                "forEach",
                "reduce",
                "reduceRight",
                "join",
                "push",
                "pop",
                "shift",
                "unshift",
                "slice",
                "sort",
            ];
            for m in methods {
                let fn_ref = vm.alloc_native_fn(m);
                let _ = vm.set_property(Value::Object(ap), m, Value::Object(fn_ref));
            }
        }
        // Math 内置对象
        vm.math_object = Some(vm.alloc_ordinary());
        // fs 内置对象
        vm.fs_object = Some(vm.alloc_ordinary());
        // 三个原生构造器（`new` 由解释器拦截求值；instanceof 经 prototype 属性判定）
        let obj_proto = vm.object_prototype;
        // Error.prototype 独立单例（链 Object.prototype）：普通对象字面量的
        // 原型链只含 object_prototype，`{} instanceof Error` 必须为 false——
        // 曾共享 obj_proto 使任意对象 instanceof Error 恒真（http-errors
        // createError 的 props 对象被误判为 Error 实例，message 丢失）
        let err_proto = vm.alloc_ordinary_with_proto(obj_proto);
        vm.error_prototype = Some(err_proto);
        vm.error_ctor = Some(vm.alloc_native_ctor("Error", Some(err_proto)));
        // Error.prototype.constructor 挂接（对齐 RegExp 原型同款做法）：
        // `new Error('x').constructor.name === 'Error'`（Node 口径）
        if let Some(ctor) = vm.error_ctor {
            let _ = vm.set_property(Value::Object(err_proto), "constructor", Value::Object(ctor));
        }
        vm.array_ctor = Some(vm.alloc_native_ctor("Array", vm.array_prototype));
        vm.object_ctor = Some(vm.alloc_native_ctor("Object", obj_proto));
        // RegExp 构造器与原型：字面量 RegExp 对象的 source/flags/lastIndex/
        // constructor 表面经 get_property 合成，instanceof/原型链经此判定
        let regexp_proto = vm.alloc_ordinary();
        vm.regexp_ctor = Some(vm.alloc_native_ctor("RegExp", Some(regexp_proto)));
        vm.regexp_prototype = Some(regexp_proto);
        if let Some(rctor) = vm.regexp_ctor {
            let _ = vm.set_property(
                Value::Object(regexp_proto),
                "constructor",
                Value::Object(rctor),
            );
        }
        // Promise / Map 构造器与 process 全局（微任务与异步基建）
        vm.promise_ctor = Some(vm.alloc_native_ctor("Promise", obj_proto));
        vm.map_ctor = Some(vm.alloc_native_ctor("Map", obj_proto));
        vm.set_ctor = Some(vm.alloc_native_ctor("Set", obj_proto));
        // Proxy 构造器单例（静态面挂接在 register_all 之后，避免注册表被整体替换）
        // process 对象：事件面（listenerCount/listeners/on 等；depd、
        // on-finished 等真实包在模块顶层调用）
        let process_obj = vm.alloc_ordinary();
        for method in [
            "listenerCount",
            "listeners",
            "on",
            "addListener",
            "once",
            "off",
            "removeListener",
            "removeAllListeners",
            "emit",
        ] {
            let f = vm.alloc_native_fn(&format!("process.{method}"));
            let _ = vm.set_property(Value::Object(process_obj), method, Value::Object(f));
        }
        // cluster worker / fork 子进程的 IPC 面（M5.2）：Node 在 bootstrap 阶段
        // 建立通道（与是否 require('cluster') 无关），此后 `process.send` /
        // `process.disconnect` / `process.channel` 存在、`process.connected` 为 true，
        // 并由本调用点激活 IPC 事件源（通道保活语义）；primary 无通道，四者均 undefined。
        if crate::builtins::cluster::worker_setup_channel(&mut vm) {
            let f = vm.alloc_native_fn("process.send");
            let _ = vm.set_property(Value::Object(process_obj), "send", Value::Object(f));
            let d = vm.alloc_native_fn("process.disconnect");
            let _ = vm.set_property(Value::Object(process_obj), "disconnect", Value::Object(d));
            let connected = Value::Boolean(crate::builtins::cluster::worker_channel_connected());
            let _ = vm.set_property(Value::Object(process_obj), "connected", connected);
            // `process.channel`（Node 由子进程 `setupChannel` 定义，仅 worker 侧存在）：
            // `ref`/`unref`/`refCounted`/`unrefCounted` 开关通道保活（见
            // `cluster::worker_channel_object` 的偏离说明）。
            let ch = crate::builtins::cluster::worker_channel_object(&mut vm);
            let _ = vm.set_property(Value::Object(process_obj), "channel", Value::Object(ch));
        }
        vm.process_object = Some(process_obj);
        // path 内置模块（方法经 CALL_METHOD 拦截求值）
        let path_mod = vm.alloc_ordinary();
        let join_fn = vm.alloc_native_fn("path.join");
        let basename_fn = vm.alloc_native_fn("path.basename");
        let dirname_fn = vm.alloc_native_fn("path.dirname");
        let extname_fn = vm.alloc_native_fn("path.extname");
        let resolve_fn = vm.alloc_native_fn("path.resolve");
        let relative_fn = vm.alloc_native_fn("path.relative");
        let _ = vm.set_property(Value::Object(path_mod), "join", Value::Object(join_fn));
        let _ = vm.set_property(
            Value::Object(path_mod),
            "basename",
            Value::Object(basename_fn),
        );
        let _ = vm.set_property(
            Value::Object(path_mod),
            "dirname",
            Value::Object(dirname_fn),
        );
        let _ = vm.set_property(
            Value::Object(path_mod),
            "extname",
            Value::Object(extname_fn),
        );
        let _ = vm.set_property(
            Value::Object(path_mod),
            "resolve",
            Value::Object(resolve_fn),
        );
        let _ = vm.set_property(
            Value::Object(path_mod),
            "relative",
            Value::Object(relative_fn),
        );
        vm.path_module = Some(path_mod);
        // stream 内置模块
        let stream_mod = vm.alloc_ordinary();
        vm.stream_module = Some(stream_mod);
        // events 内置模块
        let events_mod = vm.alloc_ordinary();
        vm.events_module = Some(events_mod);
        // os 内置模块
        let os_mod = vm.alloc_ordinary();
        let platform_fn = vm.alloc_native_fn("os.platform");
        let homedir_fn = vm.alloc_native_fn("os.homedir");
        let tmpdir_fn = vm.alloc_native_fn("os.tmpdir");
        let eol = if cfg!(windows) { "\r\n" } else { "\n" };
        let eol_str = vm.alloc_string(eol.to_owned());
        let _ = vm.set_property(
            Value::Object(os_mod),
            "platform",
            Value::Object(platform_fn),
        );
        let _ = vm.set_property(Value::Object(os_mod), "homedir", Value::Object(homedir_fn));
        let _ = vm.set_property(Value::Object(os_mod), "tmpdir", Value::Object(tmpdir_fn));
        let _ = vm.set_property(Value::Object(os_mod), "EOL", Value::Object(eol_str));
        vm.os_module = Some(os_mod);
        // 内置库注册表：必须在全部单例（fs/path/os/process/构造器）初始化之后
        // 预热（内置模块如 fs/os 的 build 复用已建单例）
        if let Err(e) = crate::builtins::register_all(&mut vm) {
            eprintln!("[reg] register_all ERR: {e}");
        }
        // BroadcastChannel 全局类（node22 conformance 06）
        crate::builtins::broadcast_channel::register_global(&mut vm);
        // Object.prototype.hasOwnProperty 分派项（register_all 之后的注册才存活）
        crate::builtins::register_handler(
            &mut vm.builtin_registry,
            "objproto",
            "hasOwnProperty",
            objproto_has_own_property,
        );
        // Proxy 构造器静态面与 Reflect 全局对象（register_all 之后的注册才存活）
        vm.proxy_ctor = Some(vm.alloc_native_ctor("Proxy", obj_proto));
        if let Some(pctor) = vm.proxy_ctor {
            crate::builtins::reflect::setup_proxy_ctor(&mut vm, pctor);
        }
        vm.reflect_object = Some(crate::builtins::reflect::materialize(&mut vm));
        // 类型化数组体系构造器（11 种 TypedArray + ArrayBuffer/
        // SharedArrayBuffer/DataView；静态面 BYTES_PER_ELEMENT 挂构造器）
        for kind in crate::typed_array::TypedKind::all() {
            let ctor = vm.alloc_native_ctor(kind.ctor_name(), obj_proto);
            let _ = vm.set_property(
                Value::Object(ctor),
                "BYTES_PER_ELEMENT",
                Value::Number(kind.elem_size() as f64),
            );
            vm.globals
                .insert(kind.ctor_name().to_owned(), Value::Object(ctor));
        }
        let ab_ctor = vm.alloc_native_ctor("ArrayBuffer", obj_proto);
        vm.globals
            .insert("ArrayBuffer".to_owned(), Value::Object(ab_ctor));
        let sab_ctor = vm.alloc_native_ctor("SharedArrayBuffer", obj_proto);
        vm.globals
            .insert("SharedArrayBuffer".to_owned(), Value::Object(sab_ctor));
        let dv_ctor = vm.alloc_native_ctor("DataView", obj_proto);
        vm.globals
            .insert("DataView".to_owned(), Value::Object(dv_ctor));
        if std::env::var("ALUKA_REQ_DEBUG").is_ok() {
            let st = vm
                .builtin_registry
                .module("stream/promises")
                .map(|r| {
                    vm.get_property(Value::Object(r), "finished")
                        .is_ok_and(|v| v.is_object())
                })
                .unwrap_or(false);
            eprintln!(
                "[bisect] after Vm::new finished-fn={st} ptr={:p}",
                &vm.builtin_registry
            );
        }
        vm
    }

    #[inline]
    pub(crate) fn pop(&mut self) -> Result<Value, VmError> {
        self.stack.pop().ok_or(VmError::StackUnderflow)
    }

    #[inline]
    pub(crate) fn peek(&self) -> Result<Value, VmError> {
        self.stack.last().copied().ok_or(VmError::StackUnderflow)
    }

    /// 格式化值为字符串（对齐 JS 的 String(...) 与 console.log 输出语义）。
    pub fn format_value(&self, val: Value) -> String {
        match val.case() {
            ValueCase::Undefined => "undefined".to_owned(),
            ValueCase::Null => "null".to_owned(),
            ValueCase::Boolean(b) => format!("{b}"),
            ValueCase::Number(n) => js_number_to_string(n),
            ValueCase::Object(r) => {
                let idx = r.0 as usize;
                // Proxy：格式化透传 target（对齐 String(proxy) 经 get/toString trap 的语义）
                if let Some(HeapObject::Proxy { target, .. }) = self.heap.get(idx) {
                    let target = *target;
                    return self.format_value(Value::Object(target));
                }
                if idx < self.heap.len() {
                    match &self.heap[idx] {
                        HeapObject::String(s) => s.clone(),
                        HeapObject::BigInt(s) => s.clone(),
                        HeapObject::Array { elements, .. } => {
                            let items: Vec<String> =
                                elements.iter().map(|e| self.format_value(*e)).collect();
                            items.join(",")
                        }
                        HeapObject::Symbol { description, .. } => {
                            crate::symbol::symbol_display(description)
                        }
                        HeapObject::Ordinary { .. }
                        | HeapObject::Generator
                        | HeapObject::Promise { .. }
                        | HeapObject::Map { .. }
                        | HeapObject::Readable { .. }
                        | HeapObject::EventEmitter { .. } => "[object Object]".to_owned(),
                        // Proxy 已在 match 前透传 target 格式化；兜底防不可达
                        HeapObject::Proxy { .. } => "[object Object]".to_owned(),
                        HeapObject::TypedArray { .. } => "[object Object]".to_owned(),
                        HeapObject::DataView { .. } => "[object Object]".to_owned(),
                        HeapObject::ArrayBuffer { .. } => "[object Object]".to_owned(),
                        HeapObject::Closure { .. }
                        | HeapObject::NativeCtor { .. }
                        | HeapObject::NativeFn { .. }
                        | HeapObject::PromiseResolver { .. } => "[function Function]".to_owned(),
                        HeapObject::RegExp { pattern, flags } => {
                            format!("/{pattern}/{flags}")
                        }
                        HeapObject::Free => String::new(),
                    }
                } else if let Some(c) = self.current_constants.get(idx) {
                    match c {
                        Constant::String(s) => s.clone(),
                        Constant::BigInt(b) => b.clone(),
                        Constant::Number(n) => format!("{n}"),
                        Constant::Bool(b) => format!("{b}"),
                        Constant::Null => "null".to_owned(),
                    }
                } else {
                    format!("[Object {:?}]", r)
                }
            }
        }
    }

    /// console.log 专用格式化（对齐 Node.js 标准输出规范）。
    ///
    /// 数组呈现为 `[ a, b ]`（空数组 `[]`，元素 `, ` 分隔、递归同规则），
    /// BigInt 呈现为带 `n` 后缀的字面量（例如 `123n`），其余值与 [`Vm::format_value`] 一致。
    pub fn format_console_value(&self, val: Value) -> String {
        if let Some(r) = val.as_object() {
            let idx = r.0 as usize;
            if let Some(obj) = self.heap.get(idx) {
                match obj {
                    HeapObject::Array { elements, .. } => {
                        if elements.is_empty() {
                            return "[]".to_owned();
                        }
                        let items: Vec<String> = elements
                            .iter()
                            .map(|e| self.format_console_value(*e))
                            .collect();
                        return format!("[ {} ]", items.join(", "));
                    }
                    HeapObject::BigInt(s) => {
                        return format!("{s}n");
                    }
                    _ => {}
                }
            } else if let Some(Constant::BigInt(s)) = self.current_constants.get(idx) {
                return format!("{s}n");
            }
        }
        self.format_value(val)
    }

    /// JS `typeof` 语义的字符串化。
    fn typeof_value(&self, val: Value) -> String {
        match val.case() {
            ValueCase::Undefined => "undefined".to_owned(),
            ValueCase::Null => "object".to_owned(),
            ValueCase::Boolean(_) => "boolean".to_owned(),
            ValueCase::Number(_) => "number".to_owned(),
            ValueCase::Object(r) => match self.heap.get(r.0 as usize) {
                Some(HeapObject::String(_)) => "string".to_owned(),
                Some(HeapObject::BigInt(_)) => "bigint".to_owned(),
                Some(HeapObject::Symbol { .. }) => "symbol".to_owned(),
                Some(HeapObject::Proxy { .. }) => self.proxy_typeof(r),
                // PromiseResolver（resolve/reject 函数）可调用，typeof 为 function
                Some(
                    HeapObject::Closure { .. }
                    | HeapObject::NativeCtor { .. }
                    | HeapObject::NativeFn { .. }
                    | HeapObject::PromiseResolver { .. },
                ) => "function".to_owned(),
                _ => "object".to_owned(),
            },
        }
    }

    /// `++` / `--` 的 ToNumeric 递增（BigInt 保持 BigInt，对齐 Go 版 `updateNumeric`）。
    fn update_numeric(&mut self, val: Value, delta: i128) -> Value {
        if let Some(r) = val.as_object() {
            if let Some(HeapObject::BigInt(s)) = self.heap.get(r.0 as usize) {
                let n: i128 = s.parse().unwrap_or(0);
                let updated = n + delta;
                return Value::Object(self.alloc_bigint(updated.to_string()));
            }
        }
        Value::Number(to_number(val) + delta as f64)
    }

    /// 解析全局名：全局变量表优先，其次内置对象，未知名返回 `undefined`。
    /// 全局解析的 JIT 入口（`resolve_global` 为私有；helper 需跨模块调用）。
    pub(crate) fn resolve_global_for_jit(&mut self, name: &str) -> Value {
        self.resolve_global(name)
    }

    /// `Function` 构造器单例（惰性构建：NativeCtor + prototype 函数对象面；
    /// prototype 由 surface 模块统一挂载方法属性）。
    pub(crate) fn function_ctor_value(&mut self) -> Value {
        if let Some(c) = self.function_ctor {
            return Value::Object(c);
        }
        let proto = crate::builtins::surface::fn_proto(self);
        let ctor = self.alloc_native_ctor("Function", Some(proto));
        self.function_ctor = Some(ctor);
        Value::Object(ctor)
    }

    /// 原型面构造器全局（String/Boolean/Number/Set/Map/WeakSet/WeakMap/WeakRef）：
    /// 单例 NativeCtor，prototype 指向 surface 建好的原型对象（`X.prototype`
    /// 可读、方法属性可取——真实包顶层存 `String.prototype.match` 等槽位）。
    pub(crate) fn proto_ctor_value(&mut self, name: &str) -> Value {
        if let Some(c) = self.ctor_cache.get(name) {
            return Value::Object(*c);
        }
        let proto = match name {
            "String" => Some(crate::builtins::surface::str_proto(self)),
            "Symbol" => Some(crate::builtins::surface::symbol_proto(self)),
            "Boolean" => Some(crate::builtins::surface::bool_proto(self)),
            "Number" => Some(crate::builtins::surface::num_proto(self)),
            "Set" | "Map" | "WeakSet" | "WeakMap" | "WeakRef" => {
                Some(crate::builtins::surface::container_proto(self))
            }
            _ => None,
        };
        let c = self.alloc_native_ctor(name, proto);
        self.ctor_cache.insert(name.to_owned(), c);
        Value::Object(c)
    }

    /// Error 子类构造器单例（TypeError/RangeError/... 名 → NativeCtor；
    /// `new` 与无 new 直调都构造带子类 name 的 Error 实例）。
    pub(crate) fn error_subclass_ctor(&mut self, name: &str) -> Value {
        if let Some(c) = self.ctor_cache.get(name) {
            return Value::Object(*c);
        }
        // 子类 prototype 链独立 Error.prototype（如 `TypeError.prototype` 可读、
        // 错误实例 instanceof TypeError 沿链命中——曾为 None 致 prototype 缺失）
        let proto = self.error_prototype;
        let c = self.alloc_native_ctor(name, proto);
        self.ctor_cache.insert(name.to_owned(), c);
        Value::Object(c)
    }

    /// 当前执行帧（函数模板索引）归属的模块作用域下标。
    pub(crate) fn module_scope_of(&self, func_idx: i64) -> Option<usize> {
        if func_idx < 0 {
            return None;
        }
        let fi = func_idx as u32;
        self.module_scopes
            .iter()
            .position(|m| fi >= m.fn_start && fi < m.fn_start + m.fn_count)
    }

    /// CJS 注入名的模块作用域解析（无归属时回落共享全局）。
    fn resolve_cjs_injected(&self, name: &str) -> Option<Value> {
        if !crate::modules::CJS_INJECTED_NAMES.contains(&name) {
            return None;
        }
        let si = self.module_scope_of(self.current_func_idx)?;
        self.module_scopes.get(si)?.vars.get(name).copied()
    }

    pub(crate) fn resolve_global(&mut self, name: &str) -> Value {
        if let Some(v) = self.resolve_cjs_injected(name) {
            return v;
        }
        if let Some(v) = self.globals.get(name) {
            return *v;
        }
        match name {
            "undefined" => Value::Undefined,
            // 全局数值常量（`typeof NaN` 实测暴露缺失——一律 "undefined"）
            "NaN" => Value::Number(f64::NAN),
            "Infinity" => Value::Number(f64::INFINITY),
            // Node 全局 Buffer 类（未显式 require('node:buffer') 时也可用）
            "Buffer" => self
                .builtin_registry
                .module("buffer")
                .and_then(|m| self.get_property(Value::Object(m), "Buffer").ok())
                .unwrap_or(Value::Undefined),
            // Node 全局 navigator（conformance 06：对象 + hardwareConcurrency/platform）
            "navigator" => {
                let obj = self.alloc_ordinary();
                let cores = std::thread::available_parallelism()
                    .map(|n| n.get() as f64)
                    .unwrap_or(1.0);
                let _ = self.set_property(
                    Value::Object(obj),
                    "hardwareConcurrency",
                    Value::Number(cores),
                );
                let platform = if cfg!(windows) { "win32" } else { "linux" };
                let p_str = self.alloc_string(platform.to_owned());
                let _ = self.set_property(Value::Object(obj), "platform", Value::Object(p_str));
                self.globals
                    .insert("navigator".to_owned(), Value::Object(obj));
                Value::Object(obj)
            }
            "Math" => self
                .math_object
                .map(Value::Object)
                .unwrap_or(Value::Undefined),
            "Error" => self
                .error_ctor
                .map(Value::Object)
                .unwrap_or(Value::Undefined),
            // Error 子类构造器（TypeError 等：es-errors 包 `module.exports =
            // TypeError` 直接导出全局——缺失会令 `new $TypeError` 崩）
            "TypeError" | "RangeError" | "SyntaxError" | "ReferenceError" | "EvalError"
            | "URIError" => self.error_subclass_ctor(name),
            "Array" => self
                .array_ctor
                .map(Value::Object)
                .unwrap_or(Value::Undefined),
            "Object" => self
                .object_ctor
                .map(Value::Object)
                .unwrap_or(Value::Undefined),
            "RegExp" => self
                .regexp_ctor
                .map(Value::Object)
                .unwrap_or(Value::Undefined),
            "fs" => self
                .fs_object
                .map(Value::Object)
                .unwrap_or(Value::Undefined),
            "Promise" => self
                .promise_ctor
                .map(Value::Object)
                .unwrap_or(Value::Undefined),
            "Map" => self.map_ctor.map(Value::Object).unwrap_or(Value::Undefined),
            "Set" => self.set_ctor.map(Value::Object).unwrap_or(Value::Undefined),
            // Proxy 构造器（`new Proxy(t, h)`；可调用形态同语义）
            "Proxy" => self
                .proxy_ctor
                .map(Value::Object)
                .unwrap_or(Value::Undefined),
            // Reflect 全局对象（13 个规范静态方法；`Vm::new` 预物化单例）
            "Reflect" => self
                .reflect_object
                .map(Value::Object)
                .unwrap_or(Value::Undefined),
            "process" => self
                .process_object
                .map(Value::Object)
                .unwrap_or(Value::Undefined),
            "os" => self
                .os_module
                .map(Value::Object)
                .unwrap_or(Value::Undefined),
            "URL" => {
                let c = self.alloc_native_ctor("URL", None);
                Value::Object(c)
            }
            "setTimeout" => {
                let f = self.alloc_native_fn("setTimeout");
                Value::Object(f)
            }
            "setInterval" => {
                let f = self.alloc_native_fn("setInterval");
                Value::Object(f)
            }
            "setImmediate" => {
                let f = self.alloc_native_fn("setImmediate");
                Value::Object(f)
            }
            "clearTimeout" => {
                let f = self.alloc_native_fn("clearTimeout");
                Value::Object(f)
            }
            "clearInterval" => {
                let f = self.alloc_native_fn("clearInterval");
                Value::Object(f)
            }
            "Boolean" => self.proto_ctor_value("Boolean"),
            "Number" => self.proto_ctor_value("Number"),
            "WeakSet" => self.proto_ctor_value("WeakSet"),
            "WeakMap" => self.proto_ctor_value("WeakMap"),
            "WeakRef" => self.proto_ctor_value("WeakRef"),
            "queueMicrotask" => {
                let f = self.alloc_native_fn("queueMicrotask");
                Value::Object(f)
            }
            // 全局结构化克隆（`structuredClone(value[, { transfer }])`）：
            // 实现复用 worker_clone 的自描述序列化（与 worker postMessage 同源）
            "structuredClone" => {
                let f = self.alloc_native_fn("structuredClone");
                Value::Object(f)
            }
            "String" => self.proto_ctor_value("String"),
            "Symbol" => self.proto_ctor_value("Symbol"),
            "JSON" => {
                // JSON 全局对象：stringify + parse
                let obj = self.alloc_ordinary();
                let _ = self.set_property(Value::Object(obj), "_isJSON", Value::Boolean(true));
                let stringify = self.alloc_native_fn("JSON.stringify");
                let _ =
                    self.set_property(Value::Object(obj), "stringify", Value::Object(stringify));
                let parse = self.alloc_native_fn("JSON.parse");
                let _ = self.set_property(Value::Object(obj), "parse", Value::Object(parse));
                Value::Object(obj)
            }
            "require" => self
                .require_fn
                .map(Value::Object)
                .unwrap_or(Value::Undefined),
            // eval：间接求值入口（直接调用形态经专管名改写，见 eval 模块）
            "eval" => {
                let f = self.alloc_native_fn("eval");
                Value::Object(f)
            }
            // 编译器改写的直接求值形态（%aluka_direct_eval%）
            crate::eval::DIRECT_EVAL_GLOBAL => {
                let f = self.alloc_native_fn("eval.direct");
                Value::Object(f)
            }
            // Function 构造器（动态函数模板）：单例 NativeCtor，prototype
            // 挂函数对象面（toString 等；call/apply/bind 走解释器通用协议）
            "Function" => self.function_ctor_value(),
            // import.meta 元属性（CJS 内联形态经全局解析；ESM wrapper 形态
            // 由 invoke_cjs_entry 注入实例）
            // ESM import 加载器（M2.2）：__aluka_import__(source)
            "__aluka_import__" => {
                let f = self.alloc_native_fn("moduleLoader.import");
                Value::Object(f)
            }
            "__importMeta" => Value::Object(
                self.build_import_meta(
                    self.entry_file.clone(),
                    self.base_dir
                        .clone()
                        .unwrap_or_else(|| std::path::PathBuf::from(".")),
                ),
            ),
            _ => Value::Undefined,
        }
    }

    /// 原型方法挂载：不可枚举数据属性（JS 原型方法语义——`for...in` 不
    /// 泄漏原型方法；M2.4 surface 补全后 Object.prototype 11 个方法曾
    /// 使 `for (k in STATUS_CODES)` 多出原型键）。
    pub(crate) fn define_proto_method(
        &mut self,
        obj: Value,
        key: &str,
        val: Value,
    ) -> Result<(), VmError> {
        let desc = self.alloc_ordinary();
        let _ = self.set_property(Value::Object(desc), "value", val);
        let _ = self.set_property(Value::Object(desc), "writable", Value::Boolean(true));
        let _ = self.set_property(Value::Object(desc), "enumerable", Value::Boolean(false));
        let _ = self.set_property(Value::Object(desc), "configurable", Value::Boolean(true));
        self.ordinary_define_property(obj, key, Value::Object(desc))?;
        // 登记不可枚举键（for-in 过滤依据）
        if let ValueCase::Object(r) = obj.case()
            && let Some(crate::heap::HeapObject::Ordinary { non_enum, .. }) =
                self.heap.get_mut(r.0 as usize)
        {
            non_enum.insert(key.to_owned());
        }
        Ok(())
    }

    /// 写入全局绑定（模块级 require/exports 等注入用）。
    pub(crate) fn set_global(&mut self, name: &str, val: Value) {
        self.globals.insert(name.to_owned(), val);
    }

    /// 判断值是否为指定名称的原生函数。
    pub(crate) fn is_native_fn(&self, val: Value, name: &str) -> bool {
        matches!(val.case(), ValueCase::Object(r)
                if matches!(
                    self.heap.get(r.0 as usize),
                    Some(HeapObject::NativeFn { name: n, .. }) if n == name
                )
        )
    }

    /// Symbol 构造器判定：NativeFn（旧形态）或 NativeCtor 单例（原型面
    /// 补全后形态）名皆为 "Symbol"。
    pub(crate) fn is_symbol_ctor(&self, val: Value) -> bool {
        matches!(val.case(), ValueCase::Object(r)
                if matches!(
                    self.heap.get(r.0 as usize),
                    Some(HeapObject::NativeFn { name, .. })
                        | Some(HeapObject::NativeCtor { name, .. })
                        if name == "Symbol"
                )
        )
    }

    /// 堆感知的 ToNumber：堆字符串/BigInt 按内容转数值（裸 `ops::to_number`
    /// 无法读取堆，字符串一律 NaN——真实包大量依赖 `"404"` 参与算术）。
    pub(crate) fn to_number_value(&self, val: Value) -> f64 {
        if let Some(r) = val.as_object() {
            match self.heap.get(r.0 as usize) {
                Some(HeapObject::String(s)) => {
                    return parse_js_number(s);
                }
                Some(HeapObject::BigInt(b)) => {
                    return b.trim().parse::<f64>().unwrap_or(f64::NAN);
                }
                // 数组 ToNumber：先 ToString（join），再按数字串解析
                // （Number([]) === 0、Number([7]) === 7 —— 生成语料实测）
                Some(HeapObject::Array { elements, .. }) => {
                    let items: Vec<String> =
                        elements.iter().map(|e| self.format_value(*e)).collect();
                    return parse_js_number(&items.join(","));
                }
                _ => {}
            }
        }
        to_number(val)
    }

    /// 判断值是否为 BigInt 堆对象。
    pub(crate) fn is_bigint_value(&self, val: Value) -> bool {
        matches!(val.case(), ValueCase::Object(r)
                if matches!(self.heap.get(r.0 as usize), Some(HeapObject::BigInt(_)))
        )
    }

    /// 数组回调上下文：`(回调, thisArg)`（thisArg 为第二参数，未传为 undefined）。
    fn array_cb_ctx(&self, args: &[Value]) -> (Value, Value) {
        (
            args.first().copied().unwrap_or(Value::Undefined),
            args.get(1).copied().unwrap_or(Value::Undefined),
        )
    }

    /// 内容相等（字符串按内容、其余按值/句柄），数组 indexOf 族使用。
    pub(crate) fn values_content_eq(&self, a: Value, b: Value) -> bool {
        if a == b {
            return true;
        }
        crate::ops::string_values_eq(&a, &b, &self.heap)
    }

    /// 读取数组堆对象的元素快照（非数组返回空集）。
    pub(crate) fn array_elements(&self, idx: usize) -> Vec<Value> {
        match self.heap.get(idx) {
            Some(HeapObject::Array { elements, .. }) => elements.clone(),
            _ => Vec::new(),
        }
    }

    /// 数组 `flat(depth)`：按深度递归展开嵌套数组。
    fn flat_array(&self, elems: Vec<Value>, depth: f64) -> Vec<Value> {
        let mut out = Vec::with_capacity(elems.len());
        for e in elems {
            if depth >= 1.0 {
                if let Some(ar) = e.as_object() {
                    if let Some(HeapObject::Array { elements, .. }) = self.heap.get(ar.0 as usize) {
                        out.extend(self.flat_array(elements.clone(), depth - 1.0));
                        continue;
                    }
                }
            }
            out.push(e);
        }
        out
    }

    /// 调用数组原型方法的回调：this=thisArg，实参按 JS 规范 `(elem, idx, arr)`。
    pub(crate) fn invoke_array_cb(
        &mut self,
        cb: Value,
        this_arg: Value,
        cb_args: &[Value],
    ) -> Result<Value, VmError> {
        self.invoke_callable(cb, this_arg, cb_args)
    }

    /// `node:path` 轻量方法实现（平台分隔符语义；符号参数规范化处理）。
    pub(crate) fn path_method(&self, method: &str, args: &[Value]) -> String {
        use std::path::{Path, PathBuf};
        let parts: Vec<String> = args.iter().map(|v| self.format_value(*v)).collect();
        match method {
            "join" => {
                let parts: Vec<String> = parts
                    .into_iter()
                    .filter(|p| !p.is_empty() && *p != "undefined" && *p != "null")
                    .collect();
                let mut buf = PathBuf::new();
                for p in &parts {
                    buf.push(p);
                }
                self.win_leading_slash(&buf.to_string_lossy())
            }
            "basename" => {
                let p = Path::new(&parts[0]);
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                match args.get(1).map(|v| v.case()) {
                    None | Some(ValueCase::Undefined) => name,
                    // 第二参为扩展名（字符串对象）：剥离（如 ".txt"）
                    Some(ValueCase::Object(_)) => name
                        .strip_suffix(&self.format_value(*args.get(1).expect("已确认存在")))
                        .unwrap_or(&name)
                        .to_string(),
                    _ => name,
                }
            }
            "dirname" => Path::new(&parts[0])
                .parent()
                .map(|p| self.win_leading_slash(&p.to_string_lossy()))
                .unwrap_or_default(),
            "extname" => Path::new(&parts[0])
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default(),
            // relative(from, to)：Node 语义——clean 后按段找公共前缀，
            // from 剩余段上溯 `..`，再接 to 剩余段
            "relative" => {
                let norm = |v: &str| -> Vec<String> {
                    v.replace('\\', "/")
                        .split('/')
                        .filter(|s| !s.is_empty() && *s != ".")
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                };
                let from = parts.first().map(String::as_str).unwrap_or("");
                let to = parts.get(1).map(String::as_str).unwrap_or("");
                let fs = norm(from);
                let ts = norm(to);
                let mut common = 0usize;
                while common < fs.len() && common < ts.len() && fs[common] == ts[common] {
                    common += 1;
                }
                let mut out: Vec<String> = Vec::new();
                for _ in common..fs.len() {
                    out.push("..".to_owned());
                }
                out.extend(ts[common..].iter().cloned());
                let joined = if out.is_empty() {
                    String::new()
                } else {
                    out.join("/")
                };
                self.win_leading_slash(&joined)
            }
            _ => {
                // resolve：当前目录为基座
                let mut buf = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                for p in &parts {
                    buf.push(p);
                }
                self.win_leading_slash(&buf.to_string_lossy())
            }
        }
    }

    /// 路径输出的前导 `/` 转 `\`（对齐 Windows 语义的 Go filepath 输出）。
    fn win_leading_slash(&self, s: &str) -> String {
        // Windows 分隔符语义：路径输出统一为 `\`（Go filepath 对齐）
        s.replace('/', "\\")
    }

    /// `new URL(href)`：轻量解析并物化属性（protocol/host/hostname/port/pathname/
    /// search/hash/href/origin），对齐 Go `node:url` 输出。
    pub(crate) fn url_constructor(&mut self, args: &[Value]) -> Value {
        let href = match args.first() {
            Some(v) => self.format_value(*v),
            None => String::new(),
        };
        let mut properties: Vec<(&str, String)> = Vec::new();
        properties.push(("href", href.clone()));

        let (scheme, rest) = match href.split_once(':') {
            Some((s, r)) if !r.is_empty() => (format!("{s}:"), r.strip_prefix("//").unwrap_or(r)),
            _ => ("".to_owned(), href.as_str()),
        };
        properties.push(("protocol", scheme.clone()));

        // authority 到首个 / ? #
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        let tail = &rest[authority_end..];
        let (pathname, search, hash) = {
            let q = tail.find('?');
            let h = tail.find('#');
            let path_end = q.or(h).unwrap_or(tail.len());
            let pathname = &tail[..path_end];
            let search = match q {
                Some(qi) => {
                    let se = h.map(|hi| hi.min(tail.len())).unwrap_or(tail.len());
                    &tail[qi..se]
                }
                None => "",
            };
            let hash = match h {
                Some(hi) => &tail[hi..],
                None => "",
            };
            (pathname, search, hash)
        };
        properties.push(("pathname", pathname.to_owned()));
        properties.push(("search", search.to_owned()));
        properties.push(("hash", hash.to_owned()));

        let userinfo_end = authority.find('@').map(|i| i + 1).unwrap_or(0);
        let host_port = &authority[userinfo_end..];
        let (host, port) = match host_port.split_once(':') {
            Some((h, p)) => (h, p.to_owned()),
            None => (host_port, String::new()),
        };
        properties.push(("hostname", host.to_owned()));
        properties.push(("port", port.clone()));
        properties.push((
            "host",
            if port.is_empty() {
                host.to_owned()
            } else {
                format!("{host}:{port}")
            },
        ));
        properties.push((
            "origin",
            if scheme.is_empty() {
                String::new()
            } else if port.is_empty() {
                format!("{scheme}//{host}")
            } else {
                format!("{scheme}//{host}:{port}")
            },
        ));

        let obj = self.alloc_ordinary();
        for (k, v) in properties {
            let s_ref = self.alloc_string(v);
            let _ = self.set_property(Value::Object(obj), k, Value::Object(s_ref));
        }
        Value::Object(obj)
    }

    /// 判断值是否为可读流实例。
    pub(crate) fn is_readable_obj(&self, val: Value) -> bool {
        matches!(val.case(), ValueCase::Object(r)
                if matches!(self.heap.get(r.0 as usize), Some(HeapObject::Readable { .. }))
        )
    }

    /// 判断值是否为数组对象。
    pub(crate) fn is_array_value(&self, val: Value) -> bool {
        matches!(val.case(), ValueCase::Object(r)
                if matches!(self.heap.get(r.0 as usize), Some(HeapObject::Array { .. }))
        )
    }

    /// 判断值是否为堆字符串对象。
    pub(crate) fn is_string_value(&self, val: Value) -> bool {
        matches!(val.case(), ValueCase::Object(r)
                if matches!(self.heap.get(r.0 as usize), Some(HeapObject::String(_)))
        )
    }

    /// 判断值是否为正则表达式对象。
    pub(crate) fn is_regexp_obj(&self, val: Value) -> bool {
        matches!(val.case(), ValueCase::Object(r)
                if matches!(self.heap.get(r.0 as usize), Some(HeapObject::RegExp { .. }))
        )
    }

    /// `RegExp.prototype.exec` 求值：成功返回结果数组对象
    /// `[全匹配, 组1, …]`，带 `index`/`input`/`groups`（命名组）属性；
    /// 无匹配返回 `None`。
    ///
    /// `g`/`y` 标志按 JS 语义从 `lastIndex` 起步并在匹配后推进、失配后归零
    /// （状态存 [`REGEX_LAST_INDEX`]，键为 RegExp 对象句柄）。
    ///
    /// 语法错误与回溯超限都以 JS 异常值上抛（`VmError::Thrown`）。
    pub(crate) fn regexp_exec(
        &mut self,
        re: Value,
        subject: &str,
    ) -> Result<Option<Value>, VmError> {
        let ValueCase::Object(r) = re.case() else {
            return Err(VmError::LocalOutOfRange);
        };
        let (pattern, flags) = match self.heap.get(r.0 as usize) {
            Some(HeapObject::RegExp { pattern, flags }) => (pattern.clone(), flags.clone()),
            _ => return Err(VmError::LocalOutOfRange),
        };
        let global = flags.contains('g');
        let sticky = flags.contains('y');
        let compiled = aluka_regex::Regex::compile(&pattern, &flags).map_err(|e| {
            let msg = self.alloc_string(e.to_string());
            VmError::Thrown(Value::Object(msg))
        })?;
        let start = if global || sticky {
            regex_last_index(r.0).min(subject.chars().count())
        } else {
            0
        };
        let matched = compiled.find_at(subject, start).map_err(|e| {
            let msg = self.alloc_string(e.to_string());
            VmError::Thrown(Value::Object(msg))
        })?;
        let Some(m) = matched else {
            if global || sticky {
                set_regex_last_index(r.0, 0);
            }
            return Ok(None);
        };
        if global || sticky {
            set_regex_last_index(r.0, m.end);
        }
        let chars: Vec<char> = subject.chars().collect();
        let slice = |a: usize, b: usize| -> String { chars[a..b].iter().collect() };
        let mut elems = vec![Value::Object(self.alloc_string(slice(m.start, m.end)))];
        for group in &m.groups {
            let elem = match group {
                Some((a, b)) => Value::Object(self.alloc_string(slice(*a, *b))),
                None => Value::Undefined,
            };
            elems.push(elem);
        }
        let result = self.alloc_array(elems);
        let index_val = Value::Number(m.start as f64);
        let _ = self.set_property(Value::Object(result), "index", index_val);
        let input_val = Value::Object(self.alloc_string(subject.to_owned()));
        let _ = self.set_property(Value::Object(result), "input", input_val);
        // 命名组 → `groups` 对象（未参与的组不落键）
        let names: Vec<Option<String>> = compiled.group_names().to_vec();
        if names.iter().any(Option::is_some) {
            let groups = self.alloc_ordinary();
            for (gi, name) in names.iter().enumerate() {
                if let (Some(name), Some(Some((a, b)))) = (name, m.groups.get(gi)) {
                    let v = Value::Object(self.alloc_string(slice(*a, *b)));
                    let _ = self.set_property(Value::Object(groups), name, v);
                }
            }
            let _ = self.set_property(Value::Object(result), "groups", Value::Object(groups));
        } else {
            let _ = self.set_property(Value::Object(result), "groups", Value::Undefined);
        }
        Ok(Some(Value::Object(result)))
    }

    /// `new RegExp(pattern[, flags])`（无 new 直调同语义）：RegExp 实参复制
    /// pattern/flags，flags 实参覆盖；经引擎编译校验，非法抛 SyntaxError。
    pub(crate) fn construct_regexp(&mut self, args: &[Value]) -> Result<Value, VmError> {
        let (pattern, mut flags) = match args.first().map(|v| v.case()) {
            None | Some(ValueCase::Undefined) => (String::new(), String::new()),
            Some(ValueCase::Object(r)) => match self.heap.get(r.0 as usize) {
                Some(HeapObject::RegExp { pattern, flags }) => (pattern.clone(), flags.clone()),
                _ => (self.format_value(args[0]), String::new()),
            },
            Some(v) => (self.format_value(Value::from(v)), String::new()),
        };
        if let Some(f) = args.get(1) {
            if !f.is_undefined() {
                flags = self.format_value(*f);
            }
        }
        if flags.contains('g') {
            // 逐字校验：未知 flag 报错（引擎自身只认识已实现的子集）
            if flags.chars().any(|c| !"dgimsuvy".contains(c)) {
                let err = self.alloc_error_instance(&format!(
                    "Invalid flags supplied to RegExp constructor '{flags}'"
                ));
                let name = self.alloc_string("SyntaxError".to_owned());
                let _ = self.set_property(Value::Object(err), "name", Value::Object(name));
                return Err(VmError::Thrown(Value::Object(err)));
            }
        }
        if let Err(e) = aluka_regex::Regex::compile(&pattern, &flags) {
            let err = self.alloc_error_instance(&format!("Invalid regular expression: {e}"));
            let name = self.alloc_string("SyntaxError".to_owned());
            let _ = self.set_property(Value::Object(err), "name", Value::Object(name));
            return Err(VmError::Thrown(Value::Object(err)));
        }
        let regexp = HeapObject::RegExp { pattern, flags };
        let idx = self.push_object(regexp);
        Ok(Value::Object(idx))
    }

    /// `String.prototype.match(regexp)`：`g` 标志收集全部全匹配（忽略
    /// lastIndex，用独立游标），否则首个 exec 结果；无匹配返回 null。
    pub(crate) fn regexp_match_value(
        &mut self,
        re: Value,
        subject: &str,
    ) -> Result<Value, VmError> {
        let ValueCase::Object(r) = re.case() else {
            return Err(VmError::LocalOutOfRange);
        };
        let (pattern, flags) = match self.heap.get(r.0 as usize) {
            Some(HeapObject::RegExp { pattern, flags }) => (pattern.clone(), flags.clone()),
            _ => return Err(VmError::LocalOutOfRange),
        };
        let compiled = aluka_regex::Regex::compile(&pattern, &flags).map_err(|e| {
            let msg = self.alloc_string(e.to_string());
            VmError::Thrown(Value::Object(msg))
        })?;
        if !flags.contains('g') {
            return match self.regexp_exec(re, subject)? {
                Some(v) => Ok(v),
                None => Ok(Value::Null),
            };
        }
        let chars: Vec<char> = subject.chars().collect();
        let mut out = Vec::new();
        let mut cursor = 0usize;
        while cursor <= chars.len() {
            let m = compiled.find_at(subject, cursor).map_err(|e| {
                let msg = self.alloc_string(e.to_string());
                VmError::Thrown(Value::Object(msg))
            })?;
            let Some(m) = m else { break };
            out.push(Value::Object(
                self.alloc_string(chars[m.start..m.end].iter().collect()),
            ));
            cursor = if m.end == m.start { m.end + 1 } else { m.end };
        }
        if out.is_empty() {
            return Ok(Value::Null);
        }
        Ok(Value::Object(self.alloc_array(out)))
    }

    /// `String.prototype.search(regexp)`：首个匹配的 index，无匹配 -1。
    pub(crate) fn regexp_search_value(
        &mut self,
        re: Value,
        subject: &str,
    ) -> Result<Value, VmError> {
        let ValueCase::Object(r) = re.case() else {
            return Err(VmError::LocalOutOfRange);
        };
        let (pattern, flags) = match self.heap.get(r.0 as usize) {
            Some(HeapObject::RegExp { pattern, flags }) => (pattern.clone(), flags.clone()),
            _ => return Err(VmError::LocalOutOfRange),
        };
        let compiled = aluka_regex::Regex::compile(&pattern, &flags).map_err(|e| {
            let msg = self.alloc_string(e.to_string());
            VmError::Thrown(Value::Object(msg))
        })?;
        let m = compiled.find(subject).map_err(|e| {
            let msg = self.alloc_string(e.to_string());
            VmError::Thrown(Value::Object(msg))
        })?;
        Ok(match m {
            Some(m) => Value::Number(m.start as f64),
            None => Value::Number(-1.0),
        })
    }

    /// `String.prototype.split(regexp[, limit])`：按正则切分并交织捕获组
    /// （`"a1b2".split(/(\d)/)` → `["a","1","b","2",""]`）。
    pub(crate) fn regexp_split_value(
        &mut self,
        re: Value,
        subject: &str,
        limit: Option<usize>,
    ) -> Result<Value, VmError> {
        let ValueCase::Object(r) = re.case() else {
            return Err(VmError::LocalOutOfRange);
        };
        let (pattern, flags) = match self.heap.get(r.0 as usize) {
            Some(HeapObject::RegExp { pattern, flags }) => (pattern.clone(), flags.clone()),
            _ => return Err(VmError::LocalOutOfRange),
        };
        // split 语义与 g 标志无关：全局迭代
        let flags = format!("{flags}g");
        let compiled = aluka_regex::Regex::compile(&pattern, &flags).map_err(|e| {
            let msg = self.alloc_string(e.to_string());
            VmError::Thrown(Value::Object(msg))
        })?;
        let chars: Vec<char> = subject.chars().collect();
        let slice = |a: usize, b: usize| -> String { chars[a..b].iter().collect() };
        let mut out: Vec<Value> = Vec::new();
        let mut cursor = 0usize;
        let mut last_end = 0usize;
        while cursor <= chars.len() {
            if let Some(limit) = limit {
                if out.len() >= limit {
                    break;
                }
            }
            let m = compiled.find_at(subject, cursor).map_err(|e| {
                let msg = self.alloc_string(e.to_string());
                VmError::Thrown(Value::Object(msg))
            })?;
            let Some(m) = m else { break };
            if m.end == m.start && m.start == last_end {
                // 空匹配紧跟上一分隔点：仅前进游标
                cursor += 1;
                continue;
            }
            out.push(Value::Object(self.alloc_string(slice(last_end, m.start))));
            for g in &m.groups {
                let elem = match g {
                    Some((a, b)) => Value::Object(self.alloc_string(slice(*a, *b))),
                    None => Value::Undefined,
                };
                out.push(elem);
            }
            last_end = m.end;
            cursor = if m.end == m.start { m.end + 1 } else { m.end };
        }
        if limit.is_none_or(|l| out.len() < l) {
            out.push(Value::Object(
                self.alloc_string(slice(last_end, chars.len())),
            ));
        }
        Ok(Value::Object(self.alloc_array(out)))
    }

    /// 执行指令序列，返回 `Return` 或 `ReturnUndef` 携带的值。
    pub fn run(&mut self, code: &[Instr]) -> Result<Value, VmError> {
        self.run_with_constants(code, &[])
    }

    /// 携带常量池执行指令序列。
    ///
    /// 扮演异常展开边界：本帧无 handler 接住的 [`VmError::Thrown`] 会继续向上
    /// （调用者帧的 `invoke_function` 调用点）传播，与 Go 版 `jsThrow` 逐帧上抛一致。
    pub fn run_with_constants(
        &mut self,
        code: &[Instr],
        constants: &[Constant],
    ) -> Result<Value, VmError> {
        // 外部低频入口：借用切片包装为 Rc（内部热路径走 run_with_constants_rc）
        self.run_with_constants_rc(code, std::rc::Rc::new(constants.to_vec()), 0)
    }

    /// 内部热路径：常量池以 `Rc` 持有（帧切换零拷贝）。
    pub(crate) fn run_with_constants_rc(
        &mut self,
        code: &[Instr],
        constants: std::rc::Rc<Vec<Constant>>,
        start_pc: usize,
    ) -> Result<Value, VmError> {
        self.run_with_constants_at(code, constants, start_pc)
    }

    /// 携带常量池从 `start_pc` 起执行指令序列（生成器挂起恢复的入口）。
    ///
    /// 扮演异常展开边界：本帧无 handler 接住的 [`VmError::Thrown`] 会继续向上
    /// （调用者帧的 `invoke_function` 调用点）传播，与 Go 版 `jsThrow` 逐帧上抛一致；
    /// [`VmError::Yielded`] 直接上抛，由生成器驱动层捕获。
    pub(crate) fn run_with_constants_at(
        &mut self,
        code: &[Instr],
        constants: std::rc::Rc<Vec<Constant>>,
        start_pc: usize,
    ) -> Result<Value, VmError> {
        self.current_constants = constants;
        let mut pc = start_pc;
        loop {
            match self.exec_frame(code, pc) {
                Ok(value) => return Ok(value),
                Err(VmError::Thrown(exc)) => match self.find_handler_in_frame(exc) {
                    // 本帧接住：从 handler 入口（catch 压入异常 / finally 直跳）续跑
                    Some(next_pc) => pc = next_pc,
                    None => return Err(VmError::Thrown(exc)),
                },
                Err(err) => {
                    // Awaited / Yielded 是正常的协程挂起信号（async/生成器由
                    // 帧收割层处理）；Exit 是 process.exit 正常终止信号，
                    // 都不是执行错误，不打日志直接上抛
                    if !matches!(
                        err,
                        VmError::Awaited(_) | VmError::Yielded(_) | VmError::Exit(_)
                    ) {
                        eprintln!(
                            "[vm-err] func={} pc={} stack={} err={err:?}",
                            self.current_func_idx,
                            self.last_pc,
                            self.stack.len()
                        );
                    }
                    return Err(err);
                }
            }
        }
    }

    /// 从 `start_pc` 起单遍执行当前帧指令流。
    ///
    /// 遇到未接住的 `Thrown` 即返回，由 [`Vm::run_with_constants`] 查找 handler
    /// 后重入续跑；嵌套调用（`invoke_function`）在返回前已恢复本帧上下文。
    fn exec_frame(&mut self, code: &[Instr], start_pc: usize) -> Result<Value, VmError> {
        let num_instrs = code.len();
        let mut pc = start_pc;
        // 当前帧常量池由 Rc 保持稳定；借用字符串键不会跨越下一次可变 VM 操作。
        let constants = std::rc::Rc::clone(&self.current_constants);

        while pc < num_instrs {
            self.last_pc = pc;
            let instr = code[pc];
            if self.coverage.is_some() {
                if let Some(cov) = self.coverage.as_mut() {
                    cov.on_instruction(pc);
                }
            }

            match instr.op {
                // 1. 标量字面量与常量加载
                Op::Nop => {}
                Op::PushUndefined => self.stack.push(Value::Undefined),
                Op::PushNull => self.stack.push(Value::Null),
                Op::PushTrue => self.stack.push(Value::Boolean(true)),
                Op::PushFalse => self.stack.push(Value::Boolean(false)),
                Op::PushInt => self.stack.push(Value::Number(f64::from(instr.operand))),
                Op::PushNegInt => self.stack.push(Value::Number(-(f64::from(instr.operand)))),
                Op::PushConst => {
                    let idx = instr.operand as usize;
                    let c = self
                        .current_constants
                        .get(idx)
                        .ok_or(VmError::LocalOutOfRange)?;
                    match c {
                        Constant::Number(n) => self.stack.push(Value::Number(*n)),
                        Constant::String(s) => {
                            let s_ref = self.alloc_string(s.clone());
                            self.stack.push(Value::Object(s_ref));
                        }
                        Constant::BigInt(b) => {
                            let b_ref = self.alloc_bigint(b.clone());
                            self.stack.push(Value::Object(b_ref));
                        }
                        Constant::Bool(b) => {
                            self.stack.push(Value::Boolean(*b));
                        }
                        Constant::Null => {
                            self.stack.push(Value::Null);
                        }
                    }
                }

                // 2. 栈操作
                Op::Pop => {
                    self.pop()?;
                }
                Op::Dup => {
                    let top = self.peek()?;
                    self.stack.push(top);
                }
                Op::Swap => {
                    let a = self.pop()?;
                    let b = self.pop()?;
                    self.stack.push(a);
                    self.stack.push(b);
                }
                // 3. 算术运算
                Op::Add => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let res = self.add_values(left, right);
                    self.stack.push(res);
                }
                Op::Sub => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    self.stack.push(Value::Number(
                        self.to_number_value(left) - self.to_number_value(right),
                    ));
                }
                Op::Mul => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    self.stack.push(Value::Number(
                        self.to_number_value(left) * self.to_number_value(right),
                    ));
                }
                Op::Div => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    self.stack.push(Value::Number(
                        self.to_number_value(left) / self.to_number_value(right),
                    ));
                }
                Op::Mod => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    self.stack.push(Value::Number(
                        self.to_number_value(left) % self.to_number_value(right),
                    ));
                }
                Op::Pow => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    self.stack.push(Value::Number(
                        self.to_number_value(left).powf(self.to_number_value(right)),
                    ));
                }
                Op::Neg => {
                    let top = self.pop()?;
                    // BigInt 取负：按十进制字符串取负生成新 BigInt（数值族走 f64）
                    if let Some(r) = top.as_object() {
                        if let Some(HeapObject::BigInt(text)) = self.heap.get(r.0 as usize) {
                            let text = text.clone();
                            let neg = if let Some(stripped) = text.strip_prefix('-') {
                                stripped.to_owned()
                            } else {
                                format!("-{text}")
                            };
                            let out = self.alloc_bigint(neg);
                            self.stack.push(Value::Object(out));
                            pc += 1;
                            continue;
                        }
                    }
                    self.stack.push(Value::Number(-self.to_number_value(top)));
                }
                Op::UnaryPlus => {
                    let top = self.pop()?;
                    self.stack.push(Value::Number(self.to_number_value(top)));
                }
                Op::Inc => {
                    let top = self.pop()?;
                    let updated = self.update_numeric(top, 1);
                    self.stack.push(updated);
                }
                Op::Dec => {
                    let top = self.pop()?;
                    let updated = self.update_numeric(top, -1);
                    self.stack.push(updated);
                }

                // 4. 位运算与逻辑非
                Op::Not => {
                    let top = self.pop()?;
                    self.stack
                        .push(Value::Boolean(!to_boolean(top, &self.heap)));
                }
                Op::BitNot => {
                    let top = self.pop()?;
                    // 位运算走**字符串感知**的 ToNumber（`~"5"` → -6）：
                    // 此前用自由函数 `to_number`（字符串一律 NaN → 0）
                    let n = self.to_number_value(top) as i32;
                    self.stack.push(Value::Number(f64::from(!n)));
                }
                Op::BitAnd => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let res =
                        (self.to_number_value(left) as i32) & (self.to_number_value(right) as i32);
                    self.stack.push(Value::Number(f64::from(res)));
                }
                Op::BitOr => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let res =
                        (self.to_number_value(left) as i32) | (self.to_number_value(right) as i32);
                    self.stack.push(Value::Number(f64::from(res)));
                }
                Op::BitXor => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let res =
                        (self.to_number_value(left) as i32) ^ (self.to_number_value(right) as i32);
                    self.stack.push(Value::Number(f64::from(res)));
                }
                Op::Shl => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let shift = (self.to_number_value(right) as i32) & 0x1f;
                    let res = (self.to_number_value(left) as i32).wrapping_shl(shift as u32);
                    self.stack.push(Value::Number(f64::from(res)));
                }
                Op::Shr => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let shift = (self.to_number_value(right) as i32) & 0x1f;
                    let res = (self.to_number_value(left) as i32).wrapping_shr(shift as u32);
                    self.stack.push(Value::Number(f64::from(res)));
                }
                Op::UShr => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let shift = (self.to_number_value(right) as i32) & 0x1f;
                    // 负数先按 i32 位型再解释为 u32（直接 `as u32` 会被
                    // Rust 的饱和转换把负数压成 0——`-16 >>> 28` 实测暴露）
                    let left = (self.to_number_value(left) as i32) as u32;
                    let res = (left.wrapping_shr(shift as u32)) as f64;
                    self.stack.push(Value::Number(res));
                }

                // 5. 比较运算
                Op::Eq => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let res = eq(left, right, &self.heap, &self.current_constants);
                    self.stack.push(Value::Boolean(res));
                }
                Op::Ne => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let res = !eq(left, right, &self.heap, &self.current_constants);
                    self.stack.push(Value::Boolean(res));
                }
                Op::StrictEq => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let res = strict_eq(left, right, &self.heap, &self.current_constants);
                    self.stack.push(Value::Boolean(res));
                }
                Op::StrictNe => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let res = !strict_eq(left, right, &self.heap, &self.current_constants);
                    self.stack.push(Value::Boolean(res));
                }
                // 关系比较：规范「抽象关系比较」——两侧皆为字符串时按 UTF-16 码元序
                // 比较，否则 ToNumber 后数值比较；任一为 NaN → 四种比较均为 false。
                // 此前一律 `to_number(a) < to_number(b)`（纯数值、且不处理字符串），
                // 导致 `"a" < "b"`、`1 < "2"` 等恒为 false。
                Op::Lt => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let res = self.js_less_than(left, right) == Some(true);
                    self.stack.push(Value::Boolean(res));
                }
                Op::Le => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    // `l <= r` ≡ `!(r < l)`，任一为 NaN 时为 false
                    let res = match self.js_less_than(right, left) {
                        Some(true) => false,
                        Some(false) => true,
                        None => false,
                    };
                    self.stack.push(Value::Boolean(res));
                }
                Op::Gt => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let res = self.js_less_than(right, left) == Some(true);
                    self.stack.push(Value::Boolean(res));
                }
                Op::Ge => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    let res = match self.js_less_than(left, right) {
                        Some(true) => false,
                        Some(false) => true,
                        None => false,
                    };
                    self.stack.push(Value::Boolean(res));
                }

                // 6. 局部变量与全局变量
                Op::LoadLocal => {
                    let slot = instr.operand as usize;

                    let val = self
                        .locals
                        .get(slot)
                        .copied()
                        .ok_or(VmError::LocalOutOfRange)?;
                    self.stack.push(val);
                }
                Op::StoreLocal => {
                    let slot = instr.operand as usize;
                    // ISA 契约：STORE_LOCAL 净栈效果 -1（弹出栈顶写入槽位）
                    let val = self.pop()?;
                    if slot >= self.locals.len() {
                        return Err(VmError::LocalOutOfRange);
                    }
                    self.locals[slot] = val;
                    // 快路径：无打开上值时跳过哈希查找（绝大多数帧）
                    if !self.open_upvalues.is_empty()
                        && let Some(uv) = self.open_upvalues.get(&slot)
                    {
                        *uv.0.borrow_mut() = val;
                    }
                }
                Op::LoadGlobal => {
                    // 操作数是常量池索引，解引用出全局对象名（对齐 Go 版 OpLoadGlobal）
                    let name = constant_string(&constants, instr.operand as usize);
                    let val = self.resolve_global(&name);
                    self.stack.push(val);
                }

                // 7. 控制流跳转
                Op::Jmp => {
                    pc = compute_jump_target(pc, instr.operand);
                    continue;
                }
                Op::JmpTruePop => {
                    let top = self.pop()?;
                    if to_boolean(top, &self.heap) {
                        pc = compute_jump_target(pc, instr.operand);
                        continue;
                    }
                }
                Op::JmpFalsePop => {
                    let top = self.pop()?;
                    if !to_boolean(top, &self.heap) {
                        pc = compute_jump_target(pc, instr.operand);
                        continue;
                    }
                }
                Op::JmpTrueKeep => {
                    let top = self.peek()?;
                    if to_boolean(top, &self.heap) {
                        pc = compute_jump_target(pc, instr.operand);
                        continue;
                    } else {
                        self.pop()?;
                    }
                }
                Op::JmpFalseKeep => {
                    let top = self.peek()?;
                    if !to_boolean(top, &self.heap) {
                        pc = compute_jump_target(pc, instr.operand);
                        continue;
                    } else {
                        self.pop()?;
                    }
                }
                Op::JmpNullishKeep => {
                    let top = self.peek()?;
                    if matches!(top, Value::Null | Value::Undefined) {
                        self.pop()?;
                    } else {
                        pc = compute_jump_target(pc, instr.operand);
                        continue;
                    }
                }
                Op::OptionalJump => {
                    let top = self.peek()?;
                    if matches!(top, Value::Null | Value::Undefined) {
                        self.pop()?;
                        self.stack.push(Value::Undefined);
                        pc = compute_jump_target(pc, instr.operand);
                        continue;
                    }
                }
                // 8. 函数调用与方法调度
                Op::CallMethod => {
                    let num_args = (instr.operand >> 16) as usize;
                    let name_idx = (instr.operand & 0xFFFF) as usize;
                    let method_name = constant_string(&constants, name_idx);

                    let mut call_args = crate::call::CallArgs::with_capacity(num_args);
                    call_args.collect_from_stack(&mut self.stack, num_args)?;
                    let args = call_args.as_slice();
                    let receiver = self.pop()?;
                    // 通用调用协议（优先于内置分派）：fn.call(thisArg, ...args) /
                    // fn.apply(thisArg, argsArray)——Function.prototype 语义，
                    // 不可被「模块名.方法名」拼接劫持。例外：解析出的方法值是
                    // Reflect./Proxy. 前缀原生函数时（如 Reflect.apply 本身即
                    // 规范静态方法），内置分派优先于通用协议
                    // bind：同样必须走 Function.prototype 语义——NativeFn
                    // receiver 的 try_dispatch 回退会错误地把 bind 分派到
                    // 函数自身方法（如 AsyncResource.runInAsyncScope.bind 被
                    // 劫持成 runInAsyncScope 调用，raw-body 依赖此形态）。
                    // 守卫：仅函数对象（Closure/NativeFn/NativeCtor）的 bind
                    // 才是 Function.prototype.bind；非函数 receiver 的 bind
                    // 是真实实例方法（dgram.Socket.bind() 等），必须走常规
                    // 方法分派——09-09 一刀切曾把 dgram bind 吞成绑定函数
                    let receiver_is_fn = matches!(receiver.case(), ValueCase::Object(rb)
                            if matches!(
                                self.heap.get(rb.0 as usize),
                                Some(HeapObject::Closure { .. })
                                    | Some(HeapObject::NativeFn { .. })
                                    | Some(HeapObject::NativeCtor { .. })
                            )
                    );
                    if method_name.as_ref() == "bind" && receiver_is_fn {
                        crate::builtins::set_current_receiver(receiver);
                        crate::builtins::set_pending_native_name("Function.prototype.bind");
                        let res = crate::builtins::surface::fn_proto_bind(self, args)?;
                        self.stack.push(res);
                        pc += 1;
                        continue;
                    }
                    if matches!(method_name.as_ref(), "call" | "apply") {
                        let method_val = self.get_property(receiver, &method_name)?;
                        let is_reflect_like = match &method_val.case() {
                            ValueCase::Object(mr) => match self.heap.get(mr.0 as usize) {
                                Some(HeapObject::NativeFn { name, .. }) => {
                                    name.starts_with("Reflect.") || name.starts_with("Proxy.")
                                }
                                _ => false,
                            },
                            _ => false,
                        };
                        if is_reflect_like {
                            if let Some(res) =
                                crate::builtins::try_dispatch(self, receiver, &method_name, args)
                            {
                                let val = res?;
                                self.stack.push(val);
                                pc += 1;
                                continue;
                            }
                        }
                        let this_arg = args.first().copied().unwrap_or(Value::Undefined);
                        let call_args: Vec<Value> = if method_name.as_ref() == "call" {
                            if args.is_empty() {
                                Vec::new()
                            } else {
                                args[1..].to_vec()
                            }
                        } else {
                            args.get(1)
                                .copied()
                                .map(|a| self.to_array_values(a))
                                .unwrap_or_default()
                        };
                        let ret = self.invoke_callable(receiver, this_arg, &call_args)?;
                        self.stack.push(ret);
                        pc += 1;
                        continue;
                    }
                    if let Some(r) = receiver.as_object() {
                        let is_reflect_like = match self.heap.get(r.0 as usize) {
                            Some(HeapObject::NativeFn { name, .. }) => {
                                name.starts_with("Reflect.") || name.starts_with("Proxy.")
                            }
                            _ => false,
                        };
                        if is_reflect_like {
                            if let Some(res) =
                                crate::builtins::try_dispatch(self, receiver, &method_name, args)
                            {
                                let val = res?;
                                self.stack.push(val);
                                pc += 1;
                                continue;
                            }
                        }
                    }
                    if let Some(res) =
                        crate::builtins::try_dispatch(self, receiver, &method_name, args)
                    {
                        let val = res?;
                        self.stack.push(val);
                    } else if method_name == "log" {
                        let line = args
                            .iter()
                            .map(|v| self.format_console_value(*v))
                            .collect::<Vec<_>>()
                            .join(" ");
                        self.stdout_records.push(line);
                        self.stack.push(Value::Undefined);
                    } else if self
                        .math_object
                        .is_some_and(|m| receiver == Value::Object(m))
                        && matches!(
                            method_name.as_ref(),
                            "abs"
                                | "ceil"
                                | "floor"
                                | "round"
                                | "trunc"
                                | "sign"
                                | "sqrt"
                                | "cbrt"
                                | "pow"
                                | "max"
                                | "min"
                                | "hypot"
                                | "log"
                                | "log2"
                                | "log10"
                                | "exp"
                                | "random"
                        )
                    {
                        // Math.*：原生方法（receiver 是 Math 单例）
                        let math_val = math_method(method_name.as_ref(), args);
                        self.stack.push(math_val);
                    } else if matches!(method_name.as_ref(), "exec" | "test")
                        && self.is_regexp_obj(receiver)
                    {
                        // RegExp 原型方法：exec 返回结果数组（带 index/input/
                        // groups）或 null；test 返回布尔（g/y 语义驱动 lastIndex）
                        let subject = args
                            .first()
                            .map(|v| self.format_value(*v))
                            .unwrap_or_default();
                        let result = self.regexp_exec(receiver, &subject)?;
                        if method_name == "test" {
                            self.stack.push(Value::Boolean(result.is_some()));
                        } else {
                            self.stack.push(result.unwrap_or(Value::Null));
                        }
                    } else if method_name == "toString" && self.is_regexp_obj(receiver) {
                        // String(re)："/pat/flags"（format_value 同形态）
                        let s = self.format_value(receiver);
                        let s_ref = self.alloc_string(s);
                        self.stack.push(Value::Object(s_ref));
                    } else if method_name == "next" && self.is_generator_obj(receiver) {
                        // 生成器迭代协议：gen.next(v) 驱动到下一个 YIELD/结束
                        let injected = args.first().copied().unwrap_or(Value::Undefined);
                        let gen_ref = match receiver.case() {
                            ValueCase::Object(r) => r,
                            _ => unreachable!("is_generator_obj 已确认 receiver 是对象"),
                        };
                        let result = self.drive_generator(gen_ref, Some(injected))?;
                        self.stack.push(result);
                    } else if method_name == "next" && self.is_array_iterator(receiver) {
                        // 数组迭代协议（for...of）：产出 { value, done } 结果对象
                        let iter_ref = match receiver.case() {
                            ValueCase::Object(r) => r,
                            _ => unreachable!("is_array_iterator 已确认 receiver 是对象"),
                        };
                        let result = self.array_iterator_next(iter_ref)?;
                        self.stack.push(result);
                    } else if method_name == "next" && self.is_string_iterator(receiver) {
                        let iter_ref = match receiver.case() {
                            ValueCase::Object(r) => r,
                            _ => unreachable!("is_string_iterator 已确认 receiver 是对象"),
                        };
                        let result = self.string_iterator_next(iter_ref)?;
                        self.stack.push(result);
                    } else if method_name == "next" && self.is_map_iterator(receiver) {
                        let iter_ref = match receiver.case() {
                            ValueCase::Object(r) => r,
                            _ => unreachable!("is_map_iterator 已确认 receiver 是对象"),
                        };
                        let result = self.map_iterator_next(iter_ref)?;
                        self.stack.push(result);
                    } else if method_name == "next" && self.is_set_iterator(receiver) {
                        let iter_ref = match receiver.case() {
                            ValueCase::Object(r) => r,
                            _ => unreachable!("is_set_iterator 已确认 receiver 是对象"),
                        };
                        let result = self.set_iterator_next(iter_ref)?;
                        self.stack.push(result);
                    } else if self.is_symbol(receiver)
                        && matches!(method_name.as_ref(), "toString" | "valueOf")
                    {
                        let sym_ref = match receiver.case() {
                            ValueCase::Object(r) => r,
                            _ => unreachable!("is_symbol 已确认 receiver 是对象"),
                        };
                        match self.call_symbol_method(&method_name, sym_ref) {
                            Some(Ok(v)) => self.stack.push(v),
                            Some(Err(e)) => return Err(e),
                            None => self.stack.push(Value::Undefined),
                        }
                    } else if self.is_bigint_value(receiver) {
                        // BigInt 原型表面：toString/toLocaleString/valueOf
                        let text = match &receiver.case() {
                            ValueCase::Object(r) => match self.heap.get(r.0 as usize) {
                                Some(HeapObject::BigInt(t)) => t.clone(),
                                _ => String::new(),
                            },
                            _ => String::new(),
                        };
                        match method_name.as_ref() {
                            "toString" => {
                                // toString(radix)：2~36 进制（默认 10）
                                let radix = args
                                    .first()
                                    .map(|v| crate::ops::to_number(*v))
                                    .unwrap_or(10.0);
                                let out = if radix == 10.0 || radix.is_nan() {
                                    text
                                } else {
                                    match text.parse::<i128>() {
                                        Ok(n) => format_radix(n, radix as u32),
                                        Err(_) => text,
                                    }
                                };
                                let s = self.alloc_string(out);
                                self.stack.push(Value::Object(s));
                            }
                            "toLocaleString" | "valueOf" => {
                                let s = self.alloc_string(text);
                                self.stack.push(Value::Object(s));
                            }
                            _ => self.stack.push(Value::Undefined),
                        }
                    } else if self.is_string_value(receiver) {
                        // 字符串原型方法：trim/indexOf/slice 等在链上直接求值
                        let text = match &receiver.case() {
                            ValueCase::Object(r) => match self.heap.get(r.0 as usize) {
                                Some(HeapObject::String(t)) => t.clone(),
                                _ => String::new(),
                            },
                            _ => String::new(),
                        };
                        match self.call_string_method(&method_name, args, &text) {
                            Some(Ok(v)) => self.stack.push(v),
                            Some(Err(e)) => return Err(e),
                            None => {
                                let msg = self.alloc_string(format!(
                                    "TypeError: {}.{} is not a function",
                                    text, method_name
                                ));
                                return Err(VmError::Thrown(Value::Object(msg)));
                            }
                        }
                    } else if method_name == "isArray"
                        && self
                            .array_ctor
                            .is_some_and(|c| receiver == Value::Object(c))
                    {
                        // Array.isArray(v)
                        let is_arr = args
                            .first()
                            .copied()
                            .map(|v| self.is_array_value(v))
                            .unwrap_or(false);
                        self.stack.push(Value::Boolean(is_arr));
                    } else if method_name == "keys"
                        && self
                            .object_ctor
                            .is_some_and(|c| receiver == Value::Object(c))
                    {
                        // Object.keys(obj)：自有可枚举键（数组为下标键；
                        // 字典序输出保证确定性；Proxy 经 ownKeys/get trap 派发）
                        let mut keys: Vec<String> = match args.first().map(|v| v.case()) {
                            Some(ValueCase::Object(r)) if self.proxy_parts(r).is_some() => {
                                self.proxy_own_keys(r).unwrap_or_default()
                            }
                            Some(ValueCase::Object(r)) => match self.heap.get(r.0 as usize) {
                                Some(HeapObject::Ordinary { .. }) => self
                                    .own_entries(r.0 as usize)
                                    .into_iter()
                                    .map(|(k, _)| k)
                                    .filter(|k| !crate::symbol::is_symbol_key(k))
                                    .collect(),
                                Some(HeapObject::Array { elements, .. }) => {
                                    (0..elements.len()).map(|i| i.to_string()).collect()
                                }
                                Some(HeapObject::Closure {
                                    properties,
                                    getters,
                                    non_enum,
                                    ..
                                }) => {
                                    // 函数对象自有面（express/body-parser 的
                                    // exports=fn + defineProperty 静态访问器；
                                    // prototype/不可枚举面过滤）
                                    let mut ks: Vec<String> = properties
                                        .keys()
                                        .filter(|k| !non_enum.contains(*k))
                                        .cloned()
                                        .collect();
                                    ks.extend(
                                        getters.keys().filter(|k| !non_enum.contains(*k)).cloned(),
                                    );
                                    ks
                                }
                                _ => Vec::new(),
                            },
                            _ => Vec::new(),
                        };
                        keys.sort();
                        let elems: Vec<Value> = keys
                            .into_iter()
                            .map(|k| {
                                let s = self.alloc_string(k);
                                Value::Object(s)
                            })
                            .collect();
                        let arr = self.alloc_array(elems);
                        self.stack.push(Value::Object(arr));
                    } else if method_name == "getOwnPropertyNames"
                        && self
                            .object_ctor
                            .is_some_and(|c| receiver == Value::Object(c))
                    {
                        // Object.getOwnPropertyNames(obj)：自有全部字符串键
                        //（含不可枚举；符号键由 getOwnPropertySymbols 返回）
                        let keys: Vec<String> = match args.first().map(|v| v.case()) {
                            Some(ValueCase::Object(r)) if self.proxy_parts(r).is_some() => {
                                self.proxy_own_keys(r).unwrap_or_default()
                            }
                            Some(ValueCase::Object(r)) => match self.heap.get(r.0 as usize) {
                                Some(HeapObject::Ordinary { .. }) => self
                                    .own_entries(r.0 as usize)
                                    .into_iter()
                                    .map(|(k, _)| k)
                                    .filter(|k| !crate::symbol::is_symbol_key(k))
                                    .collect(),
                                Some(HeapObject::Array { elements, .. }) => {
                                    let mut ks: Vec<String> =
                                        (0..elements.len()).map(|i| i.to_string()).collect();
                                    ks.extend(
                                        self.own_entries(r.0 as usize)
                                            .into_iter()
                                            .map(|(k, _)| k)
                                            .filter(|k| !crate::symbol::is_symbol_key(k)),
                                    );
                                    ks
                                }
                                Some(HeapObject::Closure {
                                    properties,
                                    getters,
                                    ..
                                }) => {
                                    let mut ks: Vec<String> = properties.keys().cloned().collect();
                                    ks.extend(getters.keys().cloned());
                                    ks
                                }
                                _ => Vec::new(),
                            },
                            _ => Vec::new(),
                        };
                        let elems: Vec<Value> = keys
                            .into_iter()
                            .map(|k| Value::Object(self.alloc_string(k)))
                            .collect();
                        let arr = self.alloc_array(elems);
                        self.stack.push(Value::Object(arr));
                    } else if matches!(method_name.as_ref(), "for" | "keyFor")
                        && self.is_symbol_ctor(receiver)
                    {
                        // Symbol.for(key) / Symbol.keyFor(sym)
                        let out = if method_name == "for" {
                            self.symbol_for(args)?
                        } else {
                            self.symbol_key_for(args)?
                        };
                        self.stack.push(out);
                    } else if method_name == "getOwnPropertySymbols"
                        && self
                            .object_ctor
                            .is_some_and(|c| receiver == Value::Object(c))
                    {
                        // Object.getOwnPropertySymbols(obj)：符号键还原为符号值
                        let syms: Vec<Value> = match args.first().map(|v| v.case()) {
                            Some(ValueCase::Object(r)) => match self.heap.get(r.0 as usize) {
                                Some(HeapObject::Ordinary { .. }) => self
                                    .own_entries(r.0 as usize)
                                    .into_iter()
                                    .map(|(k, _)| k)
                                    .filter_map(|k| crate::symbol::parse_symbol_key(&k))
                                    .map(Value::Object)
                                    .collect(),
                                _ => Vec::new(),
                            },
                            _ => Vec::new(),
                        };
                        let arr = self.alloc_array(syms);
                        self.stack.push(Value::Object(arr));
                    } else if method_name == "stringify" && self.is_json_object(receiver) {
                        // JSON.stringify(value)（成员调用形态）
                        let v = args.first().copied().unwrap_or(Value::Undefined);
                        let out = self.json_stringify(v)?;
                        self.stack.push(out);
                    } else if method_name == "parse" && self.is_json_object(receiver) {
                        // JSON.parse(text)（成员调用形态）
                        let out = self.json_parse(args)?;
                        self.stack.push(out);
                    } else if matches!(
                        method_name.as_ref(),
                        "readFileSync" | "writeFileSync" | "existsSync"
                    ) && self.fs_object.is_some_and(|f| receiver == Value::Object(f))
                    {
                        // fs 最小内置（M1）：同步读写文本文件
                        let path = args
                            .first()
                            .map(|v| self.format_value(*v))
                            .unwrap_or_default();
                        match method_name.as_ref() {
                            "existsSync" => {
                                self.stack
                                    .push(Value::Boolean(std::path::Path::new(&path).exists()));
                            }
                            "readFileSync" => match std::fs::read_to_string(&path) {
                                Ok(content) => {
                                    let s = self.alloc_string(content);
                                    self.stack.push(Value::Object(s));
                                }
                                Err(e) => {
                                    let msg = self.alloc_string(format!("fs.readFileSync: {e}"));
                                    return Err(VmError::Thrown(Value::Object(msg)));
                                }
                            },
                            _ => {
                                let data = args
                                    .get(1)
                                    .map(|v| self.format_value(*v))
                                    .unwrap_or_default();
                                match std::fs::write(&path, data) {
                                    Ok(()) => self.stack.push(Value::Undefined),
                                    Err(e) => {
                                        let msg =
                                            self.alloc_string(format!("fs.writeFileSync: {e}"));
                                        return Err(VmError::Thrown(Value::Object(msg)));
                                    }
                                }
                            }
                        }
                    } else if method_name == "nextTick" && {
                        let c1 = self
                            .process_object
                            .is_some_and(|p| receiver == Value::Object(p));
                        let c2 = matches!(receiver.case(), ValueCase::Object(rr)
                                if matches!(
                                    self.heap.get(rr.0 as usize),
                                    Some(HeapObject::NativeFn { name, .. })
                                        if name == "nextTick"
                                )
                        );
                        let _ = (c1, c2);
                        c1 || c2
                    } {
                        // process.nextTick(cb)：nextTick 优先微任务队列
                        let cb = args.first().copied().unwrap_or(Value::Undefined);
                        self.nexttick_queue.push_back(cb);
                        self.stack.push(Value::Undefined);
                    } else if matches!(method_name.as_ref(), "then" | "catch" | "finally")
                        && matches!(receiver.case(), ValueCase::Object(rr)
                                if matches!(
                                    self.heap.get(rr.0 as usize),
                                    Some(HeapObject::Promise { .. })
                                )
                        )
                    {
                        // then(onF, onR) / catch(onR) / finally(cb)：创建新 promise P2，
                        // 登记反应（pending）或立即调度（已定型）——回调返回值采纳进
                        // P2，回调抛错拒绝 P2，finally 透传原定型值
                        let cb = args.first().copied().unwrap_or(Value::Undefined);
                        let on_rejected = if method_name == "then" {
                            args.get(1).copied().unwrap_or(Value::Undefined)
                        } else {
                            Value::Undefined
                        };
                        if let Some(rr) = receiver.as_object() {
                            let p2 = self.alloc_pending_promise();
                            let res2 = self.alloc_promise_resolver(p2, true);
                            let rej2 = self.alloc_promise_resolver(p2, false);
                            let (on_f, on_r) = match method_name.as_ref() {
                                "then" => (cb, on_rejected),
                                "catch" => (Value::Undefined, cb),
                                _ => (cb, cb),
                            };
                            let is_finally = method_name == "finally";
                            let state = match self.heap.get(rr.0 as usize) {
                                Some(HeapObject::Promise {
                                    pending,
                                    value,
                                    is_rejected,
                                    ..
                                }) => Some((*pending, *value, *is_rejected)),
                                _ => None,
                            };
                            match state {
                                Some((true, _, _)) => {
                                    // pending：登记反应，定型时经 take_reactions 派发
                                    crate::builtins::promise::push_reaction(
                                        rr.0,
                                        crate::builtins::promise::Reaction {
                                            on_f,
                                            on_r,
                                            resolver: Value::Object(res2),
                                            reject_resolver: Value::Object(rej2),
                                        },
                                    );
                                }
                                Some((false, value, is_rejected)) => {
                                    // 已定型：立即调度反应
                                    if is_finally {
                                        self.microtask_queue.push_back(
                                            crate::builtins::Job::Reaction {
                                                cb,
                                                arg: value,
                                                resolver: Value::Object(res2),
                                                reject_resolver: Value::Object(rej2),
                                                is_finally: true,
                                            },
                                        );
                                    } else if is_rejected {
                                        if !matches!(on_r, Value::Undefined) {
                                            self.microtask_queue.push_back(
                                                crate::builtins::Job::Reaction {
                                                    cb: on_r,
                                                    arg: value,
                                                    resolver: Value::Object(res2),
                                                    reject_resolver: Value::Object(rej2),
                                                    is_finally: false,
                                                },
                                            );
                                        } else {
                                            // 拒绝透传（onR 缺失）：两跳任务对齐 Go
                                            // oracle 的透传时序
                                            self.microtask_queue.push_back(
                                                crate::builtins::Job::RejectLater {
                                                    resolver: Value::Object(rej2),
                                                    arg: value,
                                                },
                                            );
                                        }
                                    } else if !matches!(on_f, Value::Undefined) {
                                        self.microtask_queue.push_back(
                                            crate::builtins::Job::Reaction {
                                                cb: on_f,
                                                arg: value,
                                                resolver: Value::Object(res2),
                                                reject_resolver: Value::Object(rej2),
                                                is_finally: false,
                                            },
                                        );
                                    } else {
                                        // 兑现透传：两跳（与拒绝透传对称）
                                        self.microtask_queue.push_back(
                                            crate::builtins::Job::ResolveLater {
                                                resolver: Value::Object(res2),
                                                arg: value,
                                            },
                                        );
                                    }
                                }
                                None => {}
                            }
                            self.stack.push(Value::Object(p2));
                        } else {
                            self.stack.push(receiver);
                        }
                    } else if matches!(method_name.as_ref(), "then" | "catch")
                        && matches!(receiver.case(), ValueCase::Object(rr)
                                if matches!(
                                    self.heap.get(rr.0 as usize),
                                    Some(HeapObject::Promise { .. })
                                )
                        )
                    {
                        // promise.then(onF)：登记处理器，返回自身；已完成时立即调度。
                        // promise.catch(onR)：pending 时登记（reject 简化同 fulfill——
                        // 本引擎无 reject 语义，fulfilled 完成不触发 catch）
                        if let Some(rr) = receiver.as_object() {
                            let cb = args.first().copied().unwrap_or(Value::Undefined);
                            // then(onF, onR) 的第二参数：rejected 处理器
                            let on_rejected = if method_name == "then" {
                                args.get(1).copied().unwrap_or(Value::Undefined)
                            } else {
                                Value::Undefined
                            };
                            let state = match self.heap.get(rr.0 as usize) {
                                Some(HeapObject::Promise {
                                    pending,
                                    value,
                                    is_rejected,
                                    ..
                                }) => Some((*pending, *value, *is_rejected)),
                                _ => None,
                            };
                            if let Some((pending, value, is_rejected)) = state {
                                if pending {
                                    let registered = if let Some(HeapObject::Promise {
                                        handlers,
                                        rejected,
                                        ..
                                    }) = self.heap.get_mut(rr.0 as usize)
                                    {
                                        if method_name == "then" {
                                            handlers.push(cb);
                                            if !matches!(on_rejected, Value::Undefined) {
                                                rejected.push(on_rejected);
                                            }
                                        } else {
                                            // catch：只在 reject 时调度（fulfill 不触发）
                                            rejected.push(cb);
                                        }
                                        true
                                    } else {
                                        false
                                    };
                                    if registered {
                                        // 写屏障：pending promise（容器）注册年轻回调
                                        self.gc_write_barrier(rr, cb);
                                    }
                                } else if is_rejected {
                                    // 已拒绝：then 的 onR / catch 的 cb 立即调度
                                    let handler = if method_name == "catch" {
                                        cb
                                    } else {
                                        on_rejected
                                    };
                                    if !matches!(handler, Value::Undefined) {
                                        self.microtask_queue
                                            .push_back(crate::builtins::Job::Call(handler, value));
                                    }
                                } else if method_name == "then" {
                                    // 已兑现：onF 立即调度
                                    self.microtask_queue
                                        .push_back(crate::builtins::Job::Call(cb, value));
                                }
                            }
                        }
                        self.stack.push(receiver);
                    } else if method_name == "resolve"
                        && self
                            .promise_ctor
                            .is_some_and(|c| receiver == Value::Object(c))
                    {
                        // Promise.resolve(v)：直接完成
                        let value = args.first().copied().unwrap_or(Value::Undefined);
                        let p = self.alloc_fulfilled_promise(value);
                        self.stack.push(Value::Object(p));
                    } else if method_name == "reject"
                        && self
                            .promise_ctor
                            .is_some_and(|c| receiver == Value::Object(c))
                    {
                        // Promise.reject(reason)：直接拒绝
                        let reason = args.first().copied().unwrap_or(Value::Undefined);
                        let p = self.alloc_rejected_promise(reason);
                        self.stack.push(Value::Object(p));
                    } else if matches!(method_name.as_ref(), "all" | "race" | "allSettled")
                        && self
                            .promise_ctor
                            .is_some_and(|c| receiver == Value::Object(c))
                    {
                        // 组合器：all/race/allSettled（any 在 Go 侧不存在，不实现）
                        let kind = match method_name.as_ref() {
                            "all" => crate::builtins::promise::CombinerKind::All,
                            "race" => crate::builtins::promise::CombinerKind::Race,
                            _ => crate::builtins::promise::CombinerKind::AllSettled,
                        };
                        let p = self.promise_combiner(kind, args)?;
                        self.stack.push(p);
                    } else if method_name == "withResolvers"
                        && self
                            .promise_ctor
                            .is_some_and(|c| receiver == Value::Object(c))
                    {
                        // Promise.withResolvers()：{ promise, resolve, reject }
                        let promise = self.alloc_pending_promise();
                        let resolve = self.alloc_promise_resolver(promise, true);
                        let reject = self.alloc_promise_resolver(promise, false);
                        let result = self.alloc_ordinary();
                        let _ = self.set_property(
                            Value::Object(result),
                            "promise",
                            Value::Object(promise),
                        );
                        let _ = self.set_property(
                            Value::Object(result),
                            "resolve",
                            Value::Object(resolve),
                        );
                        let _ = self.set_property(
                            Value::Object(result),
                            "reject",
                            Value::Object(reject),
                        );
                        self.stack.push(Value::Object(result));
                    } else if method_name == "fromAsync"
                        && self
                            .array_ctor
                            .is_some_and(|c| receiver == Value::Object(c))
                    {
                        // Array.fromAsync(iterable)：同步数组直接收集；
                        // 生成器按 next() 同步驱动（async 生成器在语料中同步产值）
                        let iterable = args.first().copied().unwrap_or(Value::Undefined);
                        let mut elems: Vec<Value> = Vec::new();
                        if let Some(it) = iterable.as_object() {
                            match self.heap.get(it.0 as usize) {
                                Some(HeapObject::Array { elements, .. }) => {
                                    elems.extend(elements.iter().copied());
                                }
                                Some(HeapObject::Generator) => {
                                    let mut done = false;
                                    let re = it;
                                    while !done {
                                        let result = self.drive_generator(re, None)?;
                                        let (val, is_done) = match result.case() {
                                            ValueCase::Object(res) => {
                                                let v =
                                                    self.get_property(Value::Object(res), "value")?;
                                                let d =
                                                    self.get_property(Value::Object(res), "done")?;
                                                (v, matches!(d.case(), ValueCase::Boolean(true)))
                                            }
                                            _ => (Value::Undefined, true),
                                        };
                                        if is_done {
                                            done = true;
                                        } else {
                                            elems.push(val);
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        let arr = self.alloc_array(elems);
                        let p = self.alloc_fulfilled_promise(Value::Object(arr));
                        self.stack.push(Value::Object(p));
                    } else if method_name == "groupBy"
                        && self
                            .object_ctor
                            .is_some_and(|c| receiver == Value::Object(c))
                    {
                        // Object.groupBy(arr, cb)：分组到普通对象
                        let cb = args.get(1).copied().unwrap_or(Value::Undefined);
                        let mut groups: std::collections::HashMap<String, Vec<Value>> =
                            std::collections::HashMap::new();
                        let elems: Vec<Value> =
                            match args.first().copied().unwrap_or(Value::Undefined).case() {
                                ValueCase::Object(rr) => match self.heap.get(rr.0 as usize) {
                                    Some(HeapObject::Array { elements, .. }) => elements.clone(),
                                    _ => Vec::new(),
                                },
                                _ => Vec::new(),
                            };
                        for (i, elem) in elems.iter().enumerate() {
                            let key_val = self.invoke_array_cb(
                                cb,
                                Value::Undefined,
                                &[*elem, Value::Number(i as f64), Value::Undefined],
                            )?;
                            let key = self.to_property_key(key_val);
                            groups.entry(key).or_default().push(*elem);
                        }
                        let result = self.alloc_ordinary();
                        for (key, items) in groups {
                            let arr = self.alloc_array(items);
                            let _ =
                                self.set_property(Value::Object(result), &key, Value::Object(arr));
                        }
                        self.stack.push(Value::Object(result));
                    } else if method_name == "groupBy"
                        && self.map_ctor.is_some_and(|c| receiver == Value::Object(c))
                    {
                        // Map.groupBy(arr, cb)：分组到 Map（键保留原值 + SameValueZero
                        // 语义；首见顺序即插入序——Vec 保序，非字符串化分组）
                        let cb = args.get(1).copied().unwrap_or(Value::Undefined);
                        let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
                        let elems: Vec<Value> =
                            match args.first().copied().unwrap_or(Value::Undefined).case() {
                                ValueCase::Object(rr) => match self.heap.get(rr.0 as usize) {
                                    Some(HeapObject::Array { elements, .. }) => elements.clone(),
                                    _ => Vec::new(),
                                },
                                _ => Vec::new(),
                            };
                        for (i, elem) in elems.iter().enumerate() {
                            let key_val = self.invoke_array_cb(
                                cb,
                                Value::Undefined,
                                &[*elem, Value::Number(i as f64), Value::Undefined],
                            )?;
                            if let Some(slot) = groups
                                .iter_mut()
                                .find(|(k, _)| self.values_same_zero(*k, key_val))
                            {
                                slot.1.push(*elem);
                            } else {
                                groups.push((key_val, vec![*elem]));
                            }
                        }
                        let mut map_entries: Vec<(Value, Value)> = Vec::new();
                        for (k, v) in groups {
                            let arr = self.alloc_array(v);
                            map_entries.push((k, Value::Object(arr)));
                        }
                        let map = self.alloc_map(map_entries);
                        self.stack.push(Value::Object(map));
                    } else if matches!(
                        method_name.as_ref(),
                        "get"
                            | "set"
                            | "has"
                            | "delete"
                            | "clear"
                            | "add"
                            | "keys"
                            | "values"
                            | "entries"
                            | "forEach"
                    ) && matches!(receiver.case(), ValueCase::Object(rr)
                            if matches!(
                                self.heap.get(rr.0 as usize),
                                Some(HeapObject::Map { .. })
                            )
                    ) {
                        // Map/Set 实例方法（键保留原始 Value + SameValueZero 查找；
                        // Set 复用 Map 变体：key=value=元素原值）
                        let method = method_name.as_ref();
                        let key = args.first().copied().unwrap_or(Value::Undefined);
                        let mut result = Value::Undefined;
                        // 迭代类方法（keys/values/entries/forEach）先取有序快照
                        // 再分配迭代器（避免与可变借用冲突）
                        let snapshot: Option<Vec<(Value, Value)>> = match method {
                            "keys" | "values" | "entries" | "forEach" => match receiver.case() {
                                ValueCase::Object(rr) => match self.heap.get(rr.0 as usize) {
                                    Some(HeapObject::Map { entries }) => Some(entries.clone()),
                                    _ => None,
                                },
                                _ => None,
                            },
                            _ => None,
                        };
                        if let Some(entries) = snapshot {
                            let is_set = self.is_set_instance(receiver);
                            if let Some(rr) = receiver.as_object() {
                                match method {
                                    "keys" => {
                                        if is_set {
                                            // Set.keys === Set.values（别名）
                                            let it = self.alloc_set_iterator(rr, "keys");
                                            result = it;
                                        } else {
                                            let it = self.alloc_map_iterator(rr, "keys");
                                            result = it;
                                        }
                                    }
                                    "values" => {
                                        if is_set {
                                            let it = self.alloc_set_iterator(rr, "values");
                                            result = it;
                                        } else {
                                            let it = self.alloc_map_iterator(rr, "values");
                                            result = it;
                                        }
                                    }
                                    "entries" => {
                                        if is_set {
                                            let it = self.alloc_set_iterator(rr, "entries");
                                            result = it;
                                        } else {
                                            let it = self.alloc_map_iterator(rr, "entries");
                                            result = it;
                                        }
                                    }
                                    "forEach" => {
                                        // Map: cb(value, key, map)；Set: cb(value, value, set)
                                        let cb = args.first().copied().unwrap_or(Value::Undefined);
                                        let this_arg =
                                            args.get(1).copied().unwrap_or(Value::Undefined);
                                        if is_set {
                                            for (_, v) in entries {
                                                self.invoke_callable(
                                                    cb,
                                                    this_arg,
                                                    &[v, v, receiver],
                                                )?;
                                            }
                                        } else {
                                            for (k, v) in entries {
                                                // 键身份：直接回传原键 Value（对象键
                                                // 必须 `seen === 原键`，不得重建字符串）
                                                self.invoke_callable(
                                                    cb,
                                                    this_arg,
                                                    &[v, k, receiver],
                                                )?;
                                            }
                                        }
                                        result = Value::Undefined;
                                    }
                                    _ => {}
                                }
                            }
                        } else if let Some(rr) = receiver.as_object() {
                            // SameValueZero 命中下标：先在**不可变**借用下求出，再进入
                            // 可变借用改写——比较需读堆判定字符串内容（本 VM 以堆对象
                            // 表示字符串，句柄不同但内容相同必须视为同键），若在
                            // `get_mut` 的闭包里比较会同时持有 &mut self.heap 与 &self。
                            let hit = match self.heap.get(rr.0 as usize) {
                                Some(HeapObject::Map { entries }) => entries
                                    .iter()
                                    .position(|(k, _)| self.values_same_zero(*k, key)),
                                _ => None,
                            };
                            if let Some(HeapObject::Map { entries }) =
                                self.heap.get_mut(rr.0 as usize)
                            {
                                match method {
                                    "set" | "add" => {
                                        let value = match method {
                                            "set" => {
                                                args.get(1).copied().unwrap_or(Value::Undefined)
                                            }
                                            _ => args.first().copied().unwrap_or(Value::Undefined),
                                        };
                                        // 有序语义：既有键命中则原位更新（保插入位置），
                                        // 否则追加末尾（Node Map/Set 插入序）
                                        if let Some(i) = hit {
                                            entries[i].1 = value;
                                        } else {
                                            entries.push((key, value));
                                        }
                                        // 写屏障：Map/Set 容器（可能已升代）写入年轻引用
                                        // ——键与值都必须分别屏障（漏键屏障会让键对象在
                                        // minor 中被误回收）
                                        self.gc_write_barrier(rr, value);
                                        self.gc_write_barrier(rr, key);
                                        result = receiver;
                                    }
                                    "get" => {
                                        result =
                                            hit.map(|i| entries[i].1).unwrap_or(Value::Undefined);
                                    }
                                    "has" => {
                                        result = Value::Boolean(hit.is_some());
                                    }
                                    "delete" => {
                                        // SameValueZero 语义下至多命中一项
                                        result = Value::Boolean(match hit {
                                            Some(i) => {
                                                entries.remove(i);
                                                true
                                            }
                                            None => false,
                                        });
                                    }
                                    "clear" => {
                                        entries.clear();
                                        result = Value::Undefined;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        self.stack.push(result);
                    } else if matches!(
                        method_name.as_ref(),
                        "on" | "once" | "off" | "removeListener" | "emit"
                    ) && matches!(receiver.case(), ValueCase::Object(rr)
                            if matches!(
                                self.heap.get(rr.0 as usize),
                                Some(HeapObject::EventEmitter { .. })
                            )
                    ) {
                        // EventEmitter：on/once 注册监听器，emit 触发，off/removeListener 移除
                        if let Some(rr) = receiver.as_object() {
                            match method_name.as_ref() {
                                "on" | "once" => {
                                    let name = args
                                        .first()
                                        .map(|v| self.to_property_key(*v))
                                        .unwrap_or_default();
                                    let cb = args.get(1).copied().unwrap_or(Value::Undefined);
                                    let once = method_name == "once";
                                    if let Some(HeapObject::EventEmitter { listeners }) =
                                        self.heap.get_mut(rr.0 as usize)
                                    {
                                        listeners.entry(name).or_default().push((cb, once));
                                        self.gc_write_barrier(rr, cb);
                                    }
                                    self.stack.push(receiver);
                                }
                                "emit" => {
                                    let name = args
                                        .first()
                                        .map(|v| self.to_property_key(*v))
                                        .unwrap_or_default();
                                    // 触发瞬间收集监听器：普通监听器保持并触发，
                                    // once 的触发前移除（只触发一次）
                                    let mut all: Vec<Value> = Vec::new();
                                    if let Some(HeapObject::EventEmitter { listeners }) =
                                        self.heap.get_mut(rr.0 as usize)
                                    {
                                        if let Some(list) = listeners.get_mut(&name) {
                                            let mut fired = Vec::new();
                                            let mut keep = Vec::with_capacity(list.len());
                                            for (cb, once) in std::mem::take(list) {
                                                if once {
                                                    fired.push(cb);
                                                } else {
                                                    keep.push((cb, once));
                                                    all.push(cb);
                                                }
                                            }
                                            *list = keep;
                                            all.extend(fired);
                                        }
                                    }
                                    let emit_args: Vec<Value> =
                                        args.iter().skip(1).copied().collect();
                                    for cb in all {
                                        self.invoke_callable(cb, receiver, &emit_args)?;
                                    }
                                    self.stack.push(Value::Boolean(!emit_args.is_empty()));
                                }
                                _ => {
                                    // off / removeListener：移除匹配的监听器
                                    let name = args
                                        .first()
                                        .map(|v| self.to_property_key(*v))
                                        .unwrap_or_default();
                                    let cb = args.get(1).copied().unwrap_or(Value::Undefined);
                                    if let Some(HeapObject::EventEmitter { listeners }) =
                                        self.heap.get_mut(rr.0 as usize)
                                    {
                                        if let Some(list) = listeners.get_mut(&name) {
                                            list.retain(|(c, _)| *c != cb);
                                        }
                                    }
                                    self.stack.push(receiver);
                                }
                            }
                        }
                    } else if matches!(method_name.as_ref(), "push" | "next")
                        && self.is_readable_obj(receiver)
                    {
                        // 可读流：push 追加数据（null=结束）；next 消费（空读挂起等待）
                        match method_name.as_ref() {
                            "push" => {
                                let v = args.first().copied().unwrap_or(Value::Undefined);
                                let is_end = matches!(v, Value::Null);
                                let waiting = if let Some(rr) = receiver.as_object() {
                                    if let Some(HeapObject::Readable {
                                        buffer,
                                        ended,
                                        waiting,
                                    }) = self.heap.get_mut(rr.0 as usize)
                                    {
                                        if is_end {
                                            *ended = true;
                                        } else if waiting.is_none() {
                                            // 无等待读取者：数据入缓冲；有等待者时
                                            // 数据直接交给等待的 next（避免双读）
                                            buffer.push_back(v);
                                            // 写屏障在 get_mut 借用结束后执行（下方）
                                        }
                                        waiting.take()
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                };
                                // 写屏障：老可读流缓冲/等待槽写入新值
                                if let Some(rr2) = receiver.as_object() {
                                    self.gc_write_barrier(rr2, v);
                                }
                                // 有等待中的 promise：兑现为 {value, done} 结果对象
                                if let Some(wp) = waiting {
                                    let res_obj = self.alloc_ordinary();
                                    let done = is_end;
                                    let val = if done { Value::Undefined } else { v };
                                    let _ = self.set_property(Value::Object(res_obj), "value", val);
                                    let _ = self.set_property(
                                        Value::Object(res_obj),
                                        "done",
                                        Value::Boolean(done),
                                    );
                                    self.fulfill_promise(wp, Value::Object(res_obj))?;
                                }
                                self.stack.push(Value::Boolean(true));
                            }
                            "next" => {
                                // 先取动作：Some(值) / Done / NeedWait(等待 promise)
                                enum NextAction {
                                    Data(Value),
                                    Done,
                                    NeedWait,
                                }
                                // 无条件先建 pending promise（NeedWait 时登记等待；
                                // Data/Done 时弃用——堆对象无副作用）
                                let pending_promise = self.alloc_pending_promise();
                                let action = if let Some(rr) = receiver.as_object() {
                                    match self.heap.get_mut(rr.0 as usize) {
                                        Some(HeapObject::Readable {
                                            buffer,
                                            ended,
                                            waiting,
                                        }) => {
                                            if let Some(v) = buffer.pop_front() {
                                                NextAction::Data(v)
                                            } else if *ended {
                                                NextAction::Done
                                            } else {
                                                // 空读未结束：登记等待 promise（挂起等待 push）
                                                *waiting = Some(pending_promise);
                                                if let Some(rr2) = receiver.as_object() {
                                                    self.gc_write_barrier(
                                                        rr2,
                                                        Value::Object(pending_promise),
                                                    );
                                                }
                                                NextAction::NeedWait
                                            }
                                        }
                                        _ => NextAction::Done,
                                    }
                                } else {
                                    NextAction::Done
                                };
                                let result = match action {
                                    NextAction::Data(v) => {
                                        let res_obj = self.alloc_ordinary();
                                        let _ =
                                            self.set_property(Value::Object(res_obj), "value", v);
                                        let _ = self.set_property(
                                            Value::Object(res_obj),
                                            "done",
                                            Value::Boolean(false),
                                        );
                                        Some(res_obj)
                                    }
                                    NextAction::Done => {
                                        let res_obj = self.alloc_ordinary();
                                        let _ = self.set_property(
                                            Value::Object(res_obj),
                                            "value",
                                            Value::Undefined,
                                        );
                                        let _ = self.set_property(
                                            Value::Object(res_obj),
                                            "done",
                                            Value::Boolean(true),
                                        );
                                        Some(res_obj)
                                    }
                                    NextAction::NeedWait => None, // pending：等待 push 兑现
                                };
                                match result {
                                    Some(obj) => self.stack.push(Value::Object(obj)),
                                    None => {
                                        // 空读未结束：next 返回等待 promise 本身
                                        // （与 waiting 登记同一句柄——push 兑现它来
                                        // 恢复 async 帧），AWAIT 挂起等待 push
                                        self.stack.push(Value::Object(pending_promise));
                                    }
                                }
                            }
                            _ => self.stack.push(Value::Undefined),
                        }
                    } else if matches!(method_name.as_ref(), "platform" | "homedir" | "tmpdir")
                        && self.os_module.is_some_and(|m| receiver == Value::Object(m))
                    {
                        let result = match method_name.as_ref() {
                            "platform" => if cfg!(windows) { "win32" } else { "linux" }.to_owned(),
                            "homedir" => std::env::var("USERPROFILE")
                                .or_else(|_| std::env::var("HOME"))
                                .unwrap_or_default(),
                            _ => std::env::var("TEMP")
                                .or_else(|_| std::env::var("TMPDIR"))
                                .unwrap_or_else(|_| "/tmp".to_owned()),
                        };
                        let r = self.alloc_string(result);
                        self.stack.push(Value::Object(r));
                    } else if matches!(
                        method_name.as_ref(),
                        "join" | "basename" | "dirname" | "extname" | "resolve" | "relative"
                    ) && self
                        .path_module
                        .is_some_and(|m| receiver == Value::Object(m))
                    {
                        // node:path 轻量内置（平台分隔符，对齐 Go `filepath` 语义）
                        let result = self.path_method(method_name.as_ref(), args);
                        let r = self.alloc_string(result);
                        self.stack.push(Value::Object(r));
                    } else if matches!(method_name.as_ref(), "isWellFormed" | "toWellFormed")
                        && matches!(receiver.case(), ValueCase::Object(rr)
                                if matches!(
                                    self.heap.get(rr.0 as usize),
                                    Some(HeapObject::String(_))
                                )
                        )
                    {
                        // 字符串完整性（Rust String 恒为合法 UTF-8）
                        if method_name == "isWellFormed" {
                            self.stack.push(Value::Boolean(true));
                        } else {
                            self.stack.push(receiver);
                        }
                    } else if matches!(
                        method_name.as_ref(),
                        "toSorted" | "toReversed" | "toSpliced" | "with"
                    ) && matches!(receiver.case(), ValueCase::Object(rr)
                            if matches!(self.heap.get(rr.0 as usize), Some(HeapObject::Array { .. }))
                    ) {
                        // ES2023 不可变数组方法：返回新数组
                        let mut elems: Vec<Value> = if let Some(rr) = receiver.as_object() {
                            if let Some(HeapObject::Array { elements, .. }) =
                                self.heap.get(rr.0 as usize)
                            {
                                elements.clone()
                            } else {
                                Vec::new()
                            }
                        } else {
                            Vec::new()
                        };
                        match method_name.as_ref() {
                            "toSorted" => {
                                let cmp = args.first().copied().unwrap_or(Value::Undefined);
                                let this_val = receiver;
                                if !matches!(cmp, Value::Undefined) {
                                    // 带比较器：数值比较器按数值序（`b-a` 负值序）
                                    elems.sort_by(|a, b| {
                                        let ord = self.invoke_array_cb(
                                            cmp,
                                            Value::Undefined,
                                            &[*a, *b, this_val],
                                        );
                                        match ord {
                                            Ok(v) => match self.to_number_value(v) {
                                                x if x < 0.0 => std::cmp::Ordering::Less,
                                                x if x > 0.0 => std::cmp::Ordering::Greater,
                                                _ => std::cmp::Ordering::Equal,
                                            },
                                            Err(_) => std::cmp::Ordering::Equal,
                                        }
                                    });
                                } else {
                                    elems.sort_by(|a, b| {
                                        self.format_value(*a).cmp(&self.format_value(*b))
                                    });
                                }
                            }
                            "toReversed" => elems.reverse(),
                            "toSpliced" => {
                                let start = args
                                    .first()
                                    .and_then(|v| match v.case() {
                                        ValueCase::Number(n) => Some(n as usize),
                                        _ => None,
                                    })
                                    .unwrap_or(0)
                                    .min(elems.len());
                                let del = args
                                    .get(1)
                                    .and_then(|v| match v.case() {
                                        ValueCase::Number(n) => Some(n as usize),
                                        _ => None,
                                    })
                                    .unwrap_or(0)
                                    .min(elems.len() - start);
                                elems.splice(start..start + del, args[2..].to_vec());
                            }
                            _ => {
                                // with(idx, val)
                                let idx = args
                                    .first()
                                    .and_then(|v| match v.case() {
                                        ValueCase::Number(n) => Some(n as usize),
                                        _ => None,
                                    })
                                    .unwrap_or(0);
                                let val = args.get(1).copied().unwrap_or(Value::Undefined);
                                if idx < elems.len() {
                                    elems[idx] = val;
                                }
                            }
                        }
                        let new_arr = self.alloc_array(elems);
                        self.stack.push(Value::Object(new_arr));
                    } else if method_name == "hasOwn"
                        && self
                            .object_ctor
                            .is_some_and(|c| receiver == Value::Object(c))
                    {
                        // Object.hasOwn(obj, key)：自有属性判定（不沿原型链）
                        let result = match (
                            args.first().copied().unwrap_or(Value::Undefined).case(),
                            args.get(1)
                                .map(|v| self.to_property_key(*v))
                                .unwrap_or_default(),
                        ) {
                            (ValueCase::Object(rr), key) => match self.heap.get(rr.0 as usize) {
                                Some(HeapObject::Ordinary { .. }) => {
                                    self.has_own_slot(rr.0 as usize, &key)
                                }
                                Some(HeapObject::Array { properties, .. }) => {
                                    key == "length" || properties.contains_key(&key)
                                }
                                _ => false,
                            },
                            _ => false,
                        };
                        self.stack.push(Value::Boolean(result));
                    } else if let Some(dispatched) =
                        crate::builtins::try_dispatch(self, receiver, &method_name, args)
                    {
                        // 内置库注册表模块方法（querystring 等并行开发模块）
                        match dispatched {
                            Ok(v) => self.stack.push(v),
                            Err(e) => return Err(e),
                        }
                    } else if method_name == "call" {
                        // 通用调用协议：fn.call(thisArg, ...args)
                        let this_arg = args.first().copied().unwrap_or(Value::Undefined);
                        let rest: &[Value] = if args.is_empty() { &[] } else { &args[1..] };
                        let ret = self.invoke_callable(receiver, this_arg, rest)?;
                        self.stack.push(ret);
                    } else if method_name == "apply" {
                        // 通用调用协议：fn.apply(thisArg, argsArray)
                        let this_arg = args.first().copied().unwrap_or(Value::Undefined);
                        let call_args = args
                            .get(1)
                            .copied()
                            .map(|a| self.to_array_values(a))
                            .unwrap_or_default();
                        let ret = self.invoke_callable(receiver, this_arg, &call_args)?;
                        self.stack.push(ret);
                    } else if method_name == "create"
                        && self
                            .object_ctor
                            .is_some_and(|c| receiver == Value::Object(c))
                    {
                        // Object.create(proto)：以精确原型分配新对象（null → 无原型）
                        let proto_val = args.first().copied().unwrap_or(Value::Undefined);
                        let proto = match proto_val.case() {
                            ValueCase::Object(p) => Some(p),
                            _ => None,
                        };
                        let obj = self.alloc_ordinary_with_exact_proto(proto);
                        self.stack.push(Value::Object(obj));
                    } else if let Some(ta_res) =
                        self.typed_array_dispatch(receiver, &method_name, args)
                    {
                        // 类型化数组 / DataView / ArrayBuffer 实例方法
                        // （返回 None 表示非本体系对象，走既有路径）
                        let val = ta_res?;
                        self.stack.push(val);
                    } else if let Some(st_res) =
                        self.typed_array_statics(receiver, &method_name, args)
                    {
                        // TypedArray 构造器静态方法（from/of/isTypedArray）
                        let val = st_res?;
                        self.stack.push(val);
                    } else if let Some(r) = receiver.as_object() {
                        let idx = r.0 as usize;
                        if idx < self.heap.len()
                            && matches!(self.heap[idx], HeapObject::Array { .. })
                        {
                            match method_name.as_ref() {
                                "push" => {
                                    for a in args {
                                        self.gc_write_barrier(r, *a);
                                    }
                                    if let Some(HeapObject::Array { elements, .. }) =
                                        self.heap.get_mut(idx)
                                    {
                                        elements.extend(args);
                                        let len = elements.len() as f64;
                                        self.stack.push(Value::Number(len));
                                    } else {
                                        self.stack.push(Value::Undefined);
                                    }
                                }
                                "pop" => {
                                    // 删末元素并返回它（空数组 → undefined）
                                    // 此前本 match 缺该分支 → 落到通用路径返回
                                    // undefined 且**不改数组**（已登记分歧转为缺陷）
                                    if let Some(HeapObject::Array { elements, .. }) =
                                        self.heap.get_mut(idx)
                                    {
                                        let out = elements.pop().unwrap_or(Value::Undefined);
                                        self.stack.push(out);
                                    } else {
                                        self.stack.push(Value::Undefined);
                                    }
                                }
                                "shift" => {
                                    // 删首元素并返回它、其余前移（空数组 → undefined）
                                    if let Some(HeapObject::Array { elements, .. }) =
                                        self.heap.get_mut(idx)
                                    {
                                        let out = if elements.is_empty() {
                                            Value::Undefined
                                        } else {
                                            elements.remove(0)
                                        };
                                        self.stack.push(out);
                                    } else {
                                        self.stack.push(Value::Undefined);
                                    }
                                }
                                "unshift" => {
                                    // 前插全部实参并返回新长度（实参顺序保持）
                                    for a in args {
                                        self.gc_write_barrier(r, *a);
                                    }
                                    if let Some(HeapObject::Array { elements, .. }) =
                                        self.heap.get_mut(idx)
                                    {
                                        for (i, a) in args.iter().enumerate() {
                                            elements.insert(i, *a);
                                        }
                                        let len = elements.len() as f64;
                                        self.stack.push(Value::Number(len));
                                    } else {
                                        self.stack.push(Value::Undefined);
                                    }
                                }
                                "map" => {
                                    let (cb, this_arg) = self.array_cb_ctx(args);
                                    let elems =
                                        if let Some(HeapObject::Array { elements, .. }) =
                                            self.heap.get(idx)
                                        {
                                            elements.clone()
                                        } else {
                                            Vec::new()
                                        };
                                    let mut new_elems = Vec::with_capacity(elems.len());
                                    let arr_obj = Value::Object(ObjectRef(idx as u32));
                                    for (elem_idx, elem) in elems.iter().enumerate() {
                                        let item_res = self.invoke_array_cb(
                                            cb,
                                            this_arg,
                                            &[*elem, Value::Number(elem_idx as f64), arr_obj],
                                        )?;
                                        new_elems.push(item_res);
                                    }
                                    let new_arr = self.alloc_array(new_elems);
                                    self.stack.push(Value::Object(new_arr));
                                }
                                "filter" => {
                                    let (cb, this_arg) = self.array_cb_ctx(args);
                                    let elems =
                                        if let Some(HeapObject::Array { elements, .. }) =
                                            self.heap.get(idx)
                                        {
                                            elements.clone()
                                        } else {
                                            Vec::new()
                                        };
                                    let arr_obj = Value::Object(ObjectRef(idx as u32));
                                    let mut kept = Vec::new();
                                    for (elem_idx, elem) in elems.iter().enumerate() {
                                        let keep = self.invoke_array_cb(
                                            cb,
                                            this_arg,
                                            &[*elem, Value::Number(elem_idx as f64), arr_obj],
                                        )?;
                                        if to_boolean(keep, &self.heap) {
                                            kept.push(*elem);
                                        }
                                    }
                                    let new_arr = self.alloc_array(kept);
                                    self.stack.push(Value::Object(new_arr));
                                }
                                "find" => {
                                    let (cb, this_arg) = self.array_cb_ctx(args);
                                    let elems =
                                        if let Some(HeapObject::Array { elements, .. }) =
                                            self.heap.get(idx)
                                        {
                                            elements.clone()
                                        } else {
                                            Vec::new()
                                        };
                                    let arr_obj = Value::Object(ObjectRef(idx as u32));
                                    let mut found = Value::Undefined;
                                    for (elem_idx, elem) in elems.iter().enumerate() {
                                        let hit = self.invoke_array_cb(
                                            cb,
                                            this_arg,
                                            &[*elem, Value::Number(elem_idx as f64), arr_obj],
                                        )?;
                                        if to_boolean(hit, &self.heap) {
                                            found = *elem;
                                            break;
                                        }
                                    }
                                    self.stack.push(found);
                                }
                                "some" => {
                                    let (cb, this_arg) = self.array_cb_ctx(args);
                                    let elems =
                                        if let Some(HeapObject::Array { elements, .. }) =
                                            self.heap.get(idx)
                                        {
                                            elements.clone()
                                        } else {
                                            Vec::new()
                                        };
                                    let arr_obj = Value::Object(ObjectRef(idx as u32));
                                    let mut any = false;
                                    for (elem_idx, elem) in elems.iter().enumerate() {
                                        let hit = self.invoke_array_cb(
                                            cb,
                                            this_arg,
                                            &[*elem, Value::Number(elem_idx as f64), arr_obj],
                                        )?;
                                        if to_boolean(hit, &self.heap) {
                                            any = true;
                                            break;
                                        }
                                    }
                                    self.stack.push(Value::Boolean(any));
                                }
                                "forEach" => {
                                    let (cb, this_arg) = self.array_cb_ctx(args);
                                    let elems =
                                        if let Some(HeapObject::Array { elements, .. }) =
                                            self.heap.get(idx)
                                        {
                                            elements.clone()
                                        } else {
                                            Vec::new()
                                        };
                                    let arr_obj = Value::Object(ObjectRef(idx as u32));
                                    for (elem_idx, elem) in elems.iter().enumerate() {
                                        self.invoke_array_cb(
                                            cb,
                                            this_arg,
                                            &[*elem, Value::Number(elem_idx as f64), arr_obj],
                                        )?;
                                    }
                                    self.stack.push(Value::Undefined);
                                }
                                "reduce" => {
                                    let cb = args.first().copied().unwrap_or(Value::Undefined);
                                    let mut acc = args.get(1).copied().unwrap_or(Value::Undefined);
                                    let elems =
                                        if let Some(HeapObject::Array { elements, .. }) =
                                            self.heap.get(idx)
                                        {
                                            elements.clone()
                                        } else {
                                            Vec::new()
                                        };
                                    let arr_obj = Value::Object(ObjectRef(idx as u32));
                                    for (elem_idx, elem) in elems.iter().enumerate() {
                                        acc = self.invoke_array_cb(
                                            cb,
                                            Value::Undefined,
                                            &[acc, *elem, Value::Number(elem_idx as f64), arr_obj],
                                        )?;
                                    }
                                    self.stack.push(acc);
                                }
                                "reduceRight" => {
                                    let cb = args.first().copied().unwrap_or(Value::Undefined);
                                    let elems =
                                        if let Some(HeapObject::Array { elements, .. }) =
                                            self.heap.get(idx)
                                        {
                                            elements.clone()
                                        } else {
                                            Vec::new()
                                        };
                                    let arr_obj = Value::Object(ObjectRef(idx as u32));
                                    // 无初始值：累加器取末元素，从倒数第二个起迭代
                                    let (mut acc, start) = match args.get(1) {
                                        Some(init) if !init.is_undefined() => (*init, elems.len()),
                                        _ => match elems.last() {
                                            Some(last) => (*last, elems.len() - 1),
                                            None if elems.is_empty() => {
                                                let msg = self.alloc_string(
                                                    "Reduce of empty array with no initial value"
                                                        .to_owned(),
                                                );
                                                return Err(VmError::Thrown(Value::Object(msg)));
                                            }
                                            None => (Value::Undefined, elems.len()),
                                        },
                                    };
                                    for elem_idx in (0..start).rev() {
                                        let elem = elems[elem_idx];
                                        acc = self.invoke_array_cb(
                                            cb,
                                            Value::Undefined,
                                            &[acc, elem, Value::Number(elem_idx as f64), arr_obj],
                                        )?;
                                    }
                                    self.stack.push(acc);
                                }
                                "join" => {
                                    let sep = if let Some(sep_val) = args.first() {
                                        self.to_property_key(*sep_val)
                                    } else {
                                        ",".to_owned()
                                    };
                                    let parts: Vec<String> =
                                        if let Some(HeapObject::Array { elements, .. }) =
                                            self.heap.get(idx)
                                        {
                                            elements.iter().map(|e| self.format_value(*e)).collect()
                                        } else {
                                            Vec::new()
                                        };
                                    let joined = parts.join(&sep);
                                    let s_ref = self.alloc_string(joined);
                                    self.stack.push(Value::Object(s_ref));
                                }
                                "slice" => {
                                    let elems =
                                        if let Some(HeapObject::Array { elements, .. }) =
                                            self.heap.get(idx)
                                        {
                                            elements.clone()
                                        } else {
                                            Vec::new()
                                        };
                                    let len = elems.len() as i64;
                                    let start_raw = match args.first().map(|v| v.case()) {
                                        Some(ValueCase::Number(n)) => n as i64,
                                        _ => 0,
                                    };
                                    let start = if start_raw < 0 {
                                        (len + start_raw).max(0) as usize
                                    } else {
                                        start_raw.min(len) as usize
                                    };
                                    let end =
                                        if let Some(n) = args.get(1).and_then(|v| v.as_number()) {
                                            let end_raw = n as i64;
                                            if end_raw < 0 {
                                                (len + end_raw).max(0) as usize
                                            } else {
                                                end_raw.min(len) as usize
                                            }
                                        } else {
                                            len as usize
                                        };
                                    let sliced = if start < end && start < elems.len() {
                                        elems[start..end.min(elems.len())].to_vec()
                                    } else {
                                        Vec::new()
                                    };
                                    let new_arr = self.alloc_array(sliced);
                                    self.stack.push(Value::Object(new_arr));
                                }
                                "sort" => {
                                    // 无比较器排序：元素字符串化后按字典序原地排序（JS 默认语义）
                                    let mut elems =
                                        if let Some(HeapObject::Array { elements, .. }) =
                                            self.heap.get(idx)
                                        {
                                            elements.clone()
                                        } else {
                                            Vec::new()
                                        };
                                    elems.sort_by(|a, b| {
                                        self.format_value(*a).cmp(&self.format_value(*b))
                                    });
                                    if let Some(HeapObject::Array { elements, .. }) =
                                        self.heap.get_mut(idx)
                                    {
                                        *elements = elems;
                                    }
                                    self.stack.push(receiver);
                                }
                                "at" => {
                                    // arr.at(i)：负下标从尾部计数（越界 → undefined）
                                    let elems = self.array_elements(idx);
                                    let len = elems.len() as f64;
                                    let n = args
                                        .first()
                                        .map(|v| crate::ops::to_number(*v))
                                        .unwrap_or(f64::NAN);
                                    let i = if n < 0.0 { len + n } else { n };
                                    let out = if i.is_nan() || i < 0.0 || i >= len {
                                        Value::Undefined
                                    } else {
                                        elems.get(i as usize).copied().unwrap_or(Value::Undefined)
                                    };
                                    self.stack.push(out);
                                }
                                "concat" => {
                                    let mut elems = self.array_elements(idx);
                                    for a in args {
                                        if let Some(ar) = a.as_object() {
                                            if let Some(HeapObject::Array { elements, .. }) =
                                                self.heap.get(ar.0 as usize)
                                            {
                                                elems.extend(elements.iter().copied());
                                                continue;
                                            }
                                        }
                                        elems.push(*a);
                                    }
                                    let new_arr = self.alloc_array(elems);
                                    self.stack.push(Value::Object(new_arr));
                                }
                                "includes" => {
                                    let elems = self.array_elements(idx);
                                    let needle = args.first().copied().unwrap_or(Value::Undefined);
                                    let mut from = args
                                        .get(1)
                                        .and_then(|v| match v.case() {
                                            ValueCase::Number(n) => Some(n),
                                            _ => None,
                                        })
                                        .unwrap_or(0.0);
                                    if from < 0.0 {
                                        from += elems.len() as f64;
                                    }
                                    let from = from.max(0.0) as usize;
                                    let found = elems[from..]
                                        .iter()
                                        .any(|e| self.values_same_zero(*e, needle));
                                    self.stack.push(Value::Boolean(found));
                                }
                                "indexOf" => {
                                    let elems = self.array_elements(idx);
                                    let needle = args.first().copied().unwrap_or(Value::Undefined);
                                    let mut from = args
                                        .get(1)
                                        .and_then(|v| match v.case() {
                                            ValueCase::Number(n) => Some(n),
                                            _ => None,
                                        })
                                        .unwrap_or(0.0);
                                    if from < 0.0 {
                                        from += elems.len() as f64;
                                    }
                                    let from = from.max(0.0) as usize;
                                    let pos = elems[from..]
                                        .iter()
                                        .position(|e| self.values_content_eq(*e, needle))
                                        .map(|p| p + from)
                                        .map(|p| p as f64)
                                        .unwrap_or(-1.0);
                                    self.stack.push(Value::Number(pos));
                                }
                                "lastIndexOf" => {
                                    let elems = self.array_elements(idx);
                                    let needle = args.first().copied().unwrap_or(Value::Undefined);
                                    let pos = elems
                                        .iter()
                                        .rposition(|e| self.values_content_eq(*e, needle))
                                        .map(|p| p as f64)
                                        .unwrap_or(-1.0);
                                    self.stack.push(Value::Number(pos));
                                }
                                "reverse" => {
                                    if let Some(HeapObject::Array { elements, .. }) =
                                        self.heap.get_mut(idx)
                                    {
                                        elements.reverse();
                                    }
                                    self.stack.push(receiver);
                                }
                                "every" => {
                                    let (cb, this_arg) = self.array_cb_ctx(args);
                                    let elems = self.array_elements(idx);
                                    let arr_obj = Value::Object(ObjectRef(idx as u32));
                                    let mut all = true;
                                    for (elem_idx, elem) in elems.iter().enumerate() {
                                        let ok = self.invoke_array_cb(
                                            cb,
                                            this_arg,
                                            &[*elem, Value::Number(elem_idx as f64), arr_obj],
                                        )?;
                                        if !self.truthy(ok) {
                                            all = false;
                                            break;
                                        }
                                    }
                                    self.stack.push(Value::Boolean(all));
                                }
                                "findIndex" => {
                                    let (cb, this_arg) = self.array_cb_ctx(args);
                                    let elems = self.array_elements(idx);
                                    let arr_obj = Value::Object(ObjectRef(idx as u32));
                                    let mut found = -1.0;
                                    for (elem_idx, elem) in elems.iter().enumerate() {
                                        let ok = self.invoke_array_cb(
                                            cb,
                                            this_arg,
                                            &[*elem, Value::Number(elem_idx as f64), arr_obj],
                                        )?;
                                        if self.truthy(ok) {
                                            found = elem_idx as f64;
                                            break;
                                        }
                                    }
                                    self.stack.push(Value::Number(found));
                                }
                                "findLast" | "findLastIndex" => {
                                    let (cb, this_arg) = self.array_cb_ctx(args);
                                    let elems = self.array_elements(idx);
                                    let arr_obj = Value::Object(ObjectRef(idx as u32));
                                    let mut hit: Option<usize> = None;
                                    for (elem_idx, elem) in elems.iter().enumerate() {
                                        let ok = self.invoke_array_cb(
                                            cb,
                                            this_arg,
                                            &[*elem, Value::Number(elem_idx as f64), arr_obj],
                                        )?;
                                        if self.truthy(ok) {
                                            hit = Some(elem_idx);
                                        }
                                    }
                                    let out = match hit {
                                        Some(i) if method_name.as_ref() == "findLast" => elems[i],
                                        Some(i) => Value::Number(i as f64),
                                        None if method_name.as_ref() == "findLast" => {
                                            Value::Undefined
                                        }
                                        None => Value::Number(-1.0),
                                    };
                                    self.stack.push(out);
                                }
                                "fill" => {
                                    let fill = args.first().copied().unwrap_or(Value::Undefined);
                                    let elems = self.array_elements(idx);
                                    let len = elems.len();
                                    let (s, e) = normalize_slice_range(args, len);
                                    if let Some(HeapObject::Array { elements, .. }) =
                                        self.heap.get_mut(idx)
                                    {
                                        for slot in elements.iter_mut().take(e).skip(s) {
                                            *slot = fill;
                                        }
                                    }
                                    self.stack.push(receiver);
                                }
                                "copyWithin" => {
                                    let elems = self.array_elements(idx);
                                    let len = elems.len();
                                    let target = slice_index(
                                        args.first().map(|v| crate::ops::to_number(*v)),
                                        len,
                                    );
                                    let start = slice_index(
                                        args.get(1).map(|v| crate::ops::to_number(*v)),
                                        len,
                                    );
                                    let end = args.get(2).map(|v| crate::ops::to_number(*v));
                                    let end = match end {
                                        Some(n) => slice_index(Some(n), len),
                                        None => len,
                                    };
                                    let count = (end - start).min(len - target);
                                    if let Some(HeapObject::Array { elements, .. }) =
                                        self.heap.get_mut(idx)
                                    {
                                        elements[target..target + count]
                                            .copy_from_slice(&elems[start..start + count]);
                                    }
                                    self.stack.push(receiver);
                                }
                                "flat" => {
                                    let depth = args
                                        .first()
                                        .map(|v| crate::ops::to_number(*v))
                                        .map(|n| if n.is_nan() { 1.0 } else { n })
                                        .unwrap_or(1.0);
                                    let elems = self.array_elements(idx);
                                    let flat = self.flat_array(elems, depth);
                                    let new_arr = self.alloc_array(flat);
                                    self.stack.push(Value::Object(new_arr));
                                }
                                "flatMap" => {
                                    let (cb, this_arg) = self.array_cb_ctx(args);
                                    let elems = self.array_elements(idx);
                                    let arr_obj = Value::Object(ObjectRef(idx as u32));
                                    let mut out = Vec::with_capacity(elems.len());
                                    for (elem_idx, elem) in elems.iter().enumerate() {
                                        let mapped = self.invoke_array_cb(
                                            cb,
                                            this_arg,
                                            &[*elem, Value::Number(elem_idx as f64), arr_obj],
                                        )?;
                                        if let Some(ar) = mapped.as_object() {
                                            if let Some(HeapObject::Array { elements, .. }) =
                                                self.heap.get(ar.0 as usize)
                                            {
                                                out.extend(elements.iter().copied());
                                                continue;
                                            }
                                        }
                                        out.push(mapped);
                                    }
                                    let new_arr = self.alloc_array(out);
                                    self.stack.push(Value::Object(new_arr));
                                }
                                "splice" => {
                                    let elems = self.array_elements(idx);
                                    let len = elems.len();
                                    let start = args
                                        .first()
                                        .map(|v| crate::ops::to_number(*v))
                                        .map(|n| {
                                            if n.is_nan() {
                                                0.0
                                            } else if n < 0.0 {
                                                (len as f64 + n).max(0.0)
                                            } else {
                                                n.min(len as f64)
                                            }
                                        })
                                        .unwrap_or(0.0)
                                        as usize;
                                    let del = match args.get(1) {
                                        Some(v) => {
                                            let n = crate::ops::to_number(*v);
                                            if n < 0.0 {
                                                0
                                            } else {
                                                (n as usize).min(len - start)
                                            }
                                        }
                                        None => len - start,
                                    };
                                    let mut removed = self.array_elements(idx);
                                    {
                                        let drained: Vec<Value> = removed
                                            .splice(
                                                start..start + del,
                                                args.get(2..).unwrap_or(&[]).to_vec(),
                                            )
                                            .collect();
                                        if let Some(HeapObject::Array { elements, .. }) =
                                            self.heap.get_mut(idx)
                                        {
                                            *elements = removed.clone();
                                        }
                                        let removed_arr = self.alloc_array(drained);
                                        self.stack.push(Value::Object(removed_arr));
                                    }
                                }
                                "keys" | "values" | "entries" => {
                                    let kind = match method_name.as_ref() {
                                        "keys" => "keys",
                                        "entries" => "entries",
                                        _ => "values",
                                    };
                                    let iter =
                                        self.alloc_array_iterator_kind(ObjectRef(idx as u32), kind);
                                    self.stack.push(iter);
                                }
                                "toString" | "toLocaleString" => {
                                    let elems = self.array_elements(idx);
                                    let items: Vec<String> = elems
                                        .iter()
                                        .map(|e| match e {
                                            e if e.is_undefined() || e.is_null() => String::new(),
                                            v => self.format_value(*v),
                                        })
                                        .collect();
                                    let s = self.alloc_string(items.join(","));
                                    self.stack.push(Value::Object(s));
                                }
                                _ => self.stack.push(Value::Undefined),
                            }
                        } else {
                            // 普通对象方法调用（原型方法绑定 IC）
                            let m_site = self.pic_site(pc);
                            let method_val = self.get_method_ic(receiver, &method_name, m_site)?;
                            if let Some(m_ref) = method_val.as_object() {
                                // Promise resolver/rejecter（Promise.withResolvers 的
                                // resolve/reject 属性）：按解析器标志兑现目标 promise
                                let resolver = match self.heap.get(m_ref.0 as usize) {
                                    Some(HeapObject::PromiseResolver { promise, resolve }) => {
                                        Some((*promise, *resolve))
                                    }
                                    _ => None,
                                };
                                if let Some((promise, resolve)) = resolver {
                                    let value = args.first().copied().unwrap_or(Value::Undefined);
                                    if resolve {
                                        self.fulfill_promise(promise, value)?;
                                    } else {
                                        self.reject_promise(promise, value)?;
                                    }
                                    self.stack.push(Value::Undefined);
                                } else {
                                    // 原生函数方法（如 node:test spy）：保持 receiver 为 this，
                                    // 经注册表分派 spy 处理器
                                    let native = match self.heap.get(m_ref.0 as usize) {
                                        Some(HeapObject::NativeFn { name, .. }) => {
                                            crate::builtins::set_pending_native_name(name);
                                            crate::builtins::set_pending_callee(method_val);
                                            self.builtin_registry.lookup(name)
                                        }
                                        _ => None,
                                    };
                                    if let Some(handler) = native {
                                        crate::builtins::set_current_receiver(receiver);
                                        let ret = handler(self, args)?;
                                        self.stack.push(ret);
                                    } else {
                                        let (f_idx, uvs) = if let Some(HeapObject::Closure {
                                            func_idx,
                                            upvalues,
                                            ..
                                        }) = self.heap.get(m_ref.0 as usize)
                                        {
                                            (Some(*func_idx), upvalues.clone())
                                        } else if (m_ref.0 as usize) < self.module_functions.len() {
                                            (Some(m_ref.0 as usize), Vec::new())
                                        } else {
                                            (None, Vec::new())
                                        };

                                        if let Some(fi) = f_idx {
                                            let ret =
                                                self.invoke_function(fi, receiver, args, uvs)?;
                                            self.stack.push(ret);
                                        } else {
                                            // 方法值不可解析为函数：按 JS 语义抛
                                            // TypeError（此前静默 undefined 掩盖缺陷）
                                            let desc = self.format_value(method_val);
                                            let err = self.alloc_error_instance(&format!(
                                                "{desc} is not a function"
                                            ));
                                            let name = self.alloc_string("TypeError".to_owned());
                                            let _ = self.set_property(
                                                Value::Object(err),
                                                "name",
                                                Value::Object(name),
                                            );
                                            return Err(VmError::Thrown(Value::Object(err)));
                                        }
                                    }
                                }
                            } else {
                                // 方法属性 undefined/非对象：同样抛 TypeError
                                let err = self.alloc_error_instance(&format!(
                                    "{method_name} is not a function"
                                ));
                                let name = self.alloc_string("TypeError".to_owned());
                                let _ = self.set_property(
                                    Value::Object(err),
                                    "name",
                                    Value::Object(name),
                                );
                                return Err(VmError::Thrown(Value::Object(err)));
                            }
                        }
                    } else {
                        // 原始值 receiver 的方法调用（JS 装箱语义）：数字/布尔
                        // 走 Number.prototype 面（toString(radix)/toFixed/...）。
                        // 字符串原始值是堆字符串由上面字符串链处理，此处仅
                        // Number/Boolean——缺省仍按 undefined 返回。
                        match receiver.case() {
                            ValueCase::Number(_) | ValueCase::Boolean(_) => {
                                crate::builtins::set_current_receiver(receiver);
                                let full = format!("Number.prototype.{method_name}");
                                crate::builtins::set_pending_native_name(&full);
                                let res =
                                    crate::builtins::surface::num_method_dispatch(self, args)?;
                                self.stack.push(res);
                            }
                            _ => self.stack.push(Value::Undefined),
                        }
                    }
                }
                Op::Call => {
                    let num_args = instr.operand as usize;
                    let mut call_args = crate::call::CallArgs::with_capacity(num_args);
                    call_args.collect_from_stack(&mut self.stack, num_args)?;
                    let args = call_args.as_slice();
                    let callee = self.pop()?;
                    if self.is_native_fn(callee, "require") {
                        // require(spec)：CJS 模块加载（缓存 + 循环依赖占位）。
                        // 模块专属实例（require_bases 登记过）相对其模块目录
                        // 解析——getter/回调延迟调用仍保持 Node 闭包捕获语义
                        let spec = args.first().copied().unwrap_or(Value::Undefined);
                        let module_base = match callee.case() {
                            ValueCase::Object(r) => self.require_bases.get(&r).cloned(),
                            _ => None,
                        };
                        let exports = match module_base {
                            Some(base) => {
                                self.require_base_stack.push(base);
                                let r = self.call_require(spec);
                                self.require_base_stack.pop();
                                r?
                            }
                            None => self.call_require(spec)?,
                        };
                        self.stack.push(exports);
                    } else if self.is_native_fn(callee, "String") {
                        // String(value)：全局字符串转换
                        let v = args.first().copied().unwrap_or(Value::Undefined);
                        let s = self.alloc_string(self.format_value(v));
                        self.stack.push(Value::Object(s));
                    } else if self.is_native_fn(callee, "JSON.stringify") {
                        let v = args.first().copied().unwrap_or(Value::Undefined);
                        let out = self.json_stringify(v)?;
                        self.stack.push(out);
                    } else if self.is_native_fn(callee, "JSON.parse") {
                        let out = self.json_parse(args)?;
                        self.stack.push(out);
                    } else if self.is_symbol_ctor(callee) {
                        // Symbol([description])：唯一符号原语
                        let sym = self.symbol_create(args);
                        self.stack.push(sym);
                    } else if let Some(r) = callee.as_object() {
                        let callee_ref = r.0 as usize;
                        let resolver = match self.heap.get(callee_ref) {
                            Some(HeapObject::PromiseResolver { promise, resolve }) => {
                                Some((*promise, *resolve))
                            }
                            _ => None,
                        };
                        if let Some((promise, resolve)) = resolver {
                            // resolve(value)/reject(reason)：按解析器标志兑现/拒绝
                            // 目标 promise 并调度处理器
                            let value = args.first().copied().unwrap_or(Value::Undefined);
                            if resolve {
                                self.fulfill_promise(promise, value)?;
                            } else {
                                self.reject_promise(promise, value)?;
                            }
                            self.stack.push(Value::Undefined);
                        } else if self.is_native_fn(Value::Object(r), "setImmediate") {
                            // setImmediate(cb)：延时 0 的单次宏任务（Node 语义，
                            // express router 的 next 链核心调度）。
                            // M5.4 切片二：`mock.timers.enable({apis:['setImmediate']})`
                            // 时由假时钟接管（只登记假队列，不写 macro_tasks）。
                            let cb = args.first().copied().unwrap_or(Value::Undefined);
                            if let Some(id) = crate::builtins::test::mock::fake_schedule(
                                cb,
                                0,
                                crate::builtins::test::mock::FakeApi::SetImmediate,
                            ) {
                                self.stack.push(id);
                            } else {
                                self.timer_counter += 1;
                                let id = self.timer_counter;
                                let last_due = self
                                    .macro_tasks
                                    .back()
                                    .map(|(_, d, _, _, _)| *d)
                                    .unwrap_or(0);
                                self.macro_tasks.push_back((id, last_due, 0, cb, false));
                                self.stack.push(Value::Number(id as f64));
                            }
                        } else if self.is_native_fn(Value::Object(r), "setTimeout")
                            || self.is_native_fn(Value::Object(r), "setInterval")
                        {
                            let delay = args
                                .get(1)
                                .and_then(|v| match v.case() {
                                    ValueCase::Number(n) => Some(n as u64),
                                    _ => None,
                                })
                                .unwrap_or(0);
                            let cb = args.first().copied().unwrap_or(Value::Undefined);
                            let repeating = self.is_native_fn(Value::Object(r), "setInterval");
                            // M5.4 切片二：假时钟接管判定（同上）。
                            let fake_api = if repeating {
                                crate::builtins::test::mock::FakeApi::SetInterval
                            } else {
                                crate::builtins::test::mock::FakeApi::SetTimeout
                            };
                            if let Some(id) =
                                crate::builtins::test::mock::fake_schedule(cb, delay, fake_api)
                            {
                                self.stack.push(id);
                            } else {
                                self.timer_counter += 1;
                                let id = self.timer_counter;
                                // 到期时间 = 队尾累计到期 + 延迟（同批注册按时间序）
                                let last_due = self
                                    .macro_tasks
                                    .back()
                                    .map(|(_, d, _, _, _)| *d)
                                    .unwrap_or(0);
                                let due = last_due + delay;
                                self.macro_tasks.push_back((id, due, delay, cb, repeating));
                                // Node 返回 Timeout/Interval 句柄；简化返回数字 id
                                // （clear* 接受数字或对象，数字自洽）
                                self.stack.push(Value::Number(id as f64));
                            }
                        } else if self.is_native_fn(Value::Object(r), "clearTimeout")
                            || self.is_native_fn(Value::Object(r), "clearInterval")
                        {
                            let id = args
                                .first()
                                .and_then(|v| match v.case() {
                                    ValueCase::Number(n) => Some(n as u64),
                                    _ => None,
                                })
                                .unwrap_or(0);
                            // M5.4 切片二：假时钟接管时清除只作用于假队列
                            // （Node `#clearTimer` 只认假句柄）。
                            let fake_api = if self.is_native_fn(Value::Object(r), "clearInterval") {
                                crate::builtins::test::mock::FakeApi::SetInterval
                            } else {
                                crate::builtins::test::mock::FakeApi::SetTimeout
                            };
                            if !crate::builtins::test::mock::fake_clear(id, fake_api) {
                                self.active_timers.insert(id);
                            }
                            self.stack.push(Value::Undefined);
                        } else if self.is_native_fn(Value::Object(r), "queueMicrotask") {
                            let cb = args.first().copied().unwrap_or(Value::Undefined);
                            self.microtask_queue
                                .push_back(crate::builtins::Job::Call(cb, Value::Undefined));
                            self.stack.push(Value::Undefined);
                        } else if self.is_native_fn(Value::Object(r), "structuredClone") {
                            // structuredClone(value[, { transfer }])：结构化克隆往返
                            // （序列化字节 → 反序列化到本堆，与 worker postMessage 同源）
                            let out = self.structured_clone(args)?;
                            self.stack.push(out);
                        } else {
                            let ret =
                                self.invoke_callable(Value::Object(r), Value::Undefined, args)?;
                            self.stack.push(ret);
                        }
                    } else {
                        // 非对象 callee（undefined/null/原始值）：JS 语义抛
                        // TypeError（此前静默返回 undefined，掩盖调用错误）
                        let ret = self.invoke_callable(callee, Value::Undefined, args)?;
                        self.stack.push(ret);
                    }
                }
                Op::CallWithThis => {
                    let num_args = instr.operand as usize;
                    let mut call_args = crate::call::CallArgs::with_capacity(num_args);
                    call_args.collect_from_stack(&mut self.stack, num_args)?;
                    let args = call_args.as_slice();
                    let this_val = self.pop()?;
                    let callee = self.pop()?;
                    let ret = self.invoke_callable(callee, this_val, args)?;
                    self.stack.push(ret);
                }

                // 9. 闭包与 Upvalues
                Op::MakeClosure => {
                    let target_func_idx = instr.operand as usize;

                    let tmpl = self
                        .module_functions
                        .get(target_func_idx)
                        .cloned()
                        .ok_or(VmError::LocalOutOfRange)?;

                    let mut captured = Vec::with_capacity(tmpl.upvalues.len());
                    for cap in &tmpl.upvalues {
                        if cap.is_local {
                            let slot = cap.index as usize;
                            let uv = self
                                .open_upvalues
                                .entry(slot)
                                .or_insert_with(|| {
                                    let val =
                                        self.locals.get(slot).copied().unwrap_or(Value::Undefined);
                                    Upvalue(std::rc::Rc::new(std::cell::RefCell::new(val)))
                                })
                                .clone();
                            captured.push(uv);
                        } else {
                            let inherited = self
                                .current_upvalues
                                .get(cap.index as usize)
                                .cloned()
                                .unwrap_or_else(|| {
                                    Upvalue(std::rc::Rc::new(std::cell::RefCell::new(
                                        Value::Undefined,
                                    )))
                                });
                            captured.push(inherited);
                        }
                    }
                    let closure_ref = self.alloc_closure_with_upvalues(target_func_idx, captured);
                    self.stack.push(Value::Object(closure_ref));
                }
                Op::LoadUpvalue => {
                    let uv_idx = instr.operand as usize;
                    let val = self
                        .current_upvalues
                        .get(uv_idx)
                        .map(|uv| *uv.0.borrow())
                        .unwrap_or(Value::Undefined);
                    self.stack.push(val);
                }
                Op::StoreUpvalue => {
                    // ISA 契约：STORE_UPVALUE 净栈效果 -1（弹出栈顶写入上值）
                    let val = self.pop()?;
                    let uv_idx = instr.operand as usize;
                    if let Some(uv) = self.current_upvalues.get(uv_idx) {
                        *uv.0.borrow_mut() = val;
                    }
                }
                Op::CloseUpvalues => {
                    let from_slot = instr.operand as usize;
                    self.open_upvalues.retain(|&slot, _| slot < from_slot);
                }

                // 10. 对象与数组字面量
                Op::NewObject => {
                    let prop_count = instr.operand as usize;
                    let obj_ref = self.alloc_ordinary();
                    if prop_count > 0 {
                        let mut pairs = Vec::with_capacity(prop_count * 2);
                        for _ in 0..(prop_count * 2) {
                            pairs.push(self.pop()?);
                        }
                        pairs.reverse();
                        for i in (0..pairs.len()).step_by(2) {
                            let k = self.to_property_key(pairs[i]);
                            let v = pairs[i + 1];
                            self.set_property(Value::Object(obj_ref), &k, v)?;
                        }
                    }
                    self.stack.push(Value::Object(obj_ref));
                }
                Op::NewArray | Op::BuildArray => {
                    let n = instr.operand as usize;
                    let mut elements = Vec::with_capacity(n);
                    for _ in 0..n {
                        elements.push(self.pop()?);
                    }
                    elements.reverse();
                    let arr_ref = self.alloc_array(elements);
                    self.stack.push(Value::Object(arr_ref));
                }
                Op::ArrayPush => {
                    let val = self.pop()?;
                    let arr_val = self.peek()?;
                    if let Some(r) = arr_val.as_object() {
                        if let Some(HeapObject::Array { elements, .. }) =
                            self.heap.get_mut(r.0 as usize)
                        {
                            elements.push(val);
                        }
                        self.gc_write_barrier(r, val);
                    }
                }
                Op::ArraySpread => {
                    let spread_val = self.pop()?;
                    let target_arr = self.peek()?;
                    // 展开语义：数组/字符串/Map/Set/类型化数组全部按迭代协议物化
                    let to_append = self.collect_iter_values(spread_val)?;
                    if let Some(t_ref) = target_arr.as_object() {
                        // 写屏障：老数组展开追加年轻元素（borrow 前先屏障）
                        for a in &to_append {
                            self.gc_write_barrier(t_ref, *a);
                        }
                        if let Some(HeapObject::Array { elements, .. }) =
                            self.heap.get_mut(t_ref.0 as usize)
                        {
                            elements.extend(to_append);
                        }
                    }
                }
                // 11. 属性操作
                Op::SetProp => {
                    let key = constant_string(&constants, instr.operand as usize);
                    let val = self.pop()?;
                    let obj = self.pop()?;
                    let site = self.pic_site(pc);
                    self.set_property_ic(obj, &key, val, site)?;
                    self.stack.push(val);
                }
                Op::SetPropObj => {
                    let key = constant_string(&constants, instr.operand as usize);
                    let val = self.pop()?;
                    let obj = self.peek()?;
                    let site = self.pic_site(pc);
                    self.set_property_ic(obj, &key, val, site)?;
                }
                Op::SetPropTop => {
                    let key = constant_string(&constants, instr.operand as usize);
                    let obj = self.pop()?;
                    let val = self.pop()?;
                    let site = self.pic_site(pc);
                    self.set_property_ic(obj, &key, val, site)?;
                }
                Op::SetPropComputedObj => {
                    let val = self.pop()?;
                    let key_val = self.pop()?;
                    let key = self.to_property_key(key_val);
                    let obj = self.peek()?;
                    self.set_property(obj, &key, val)?;
                }
                Op::GetProp => {
                    let key = constant_string(&constants, instr.operand as usize);
                    let obj = self.pop()?;
                    let site = self.pic_site(pc);

                    let val = self.get_property_ic(obj, &key, site)?;
                    self.stack.push(val);
                }
                Op::GetPropLocal => {
                    let slot = (instr.operand >> 16) as usize;
                    let name_idx = (instr.operand & 0xFFFF) as usize;
                    let key = constant_string(&constants, name_idx);
                    let obj = *self.locals.get(slot).ok_or(VmError::LocalOutOfRange)?;
                    let site = self.pic_site(pc);
                    let val = self.get_property_ic(obj, &key, site)?;
                    self.stack.push(val);
                }
                Op::GetElem => {
                    let key_val = self.pop()?;
                    let obj = self.pop()?;
                    let key = self.to_property_key(key_val);
                    let val = self.get_property(obj, &key)?;
                    self.stack.push(val);
                }
                Op::SetElem => {
                    let val = self.pop()?;
                    let key_val = self.pop()?;
                    let obj = self.pop()?;
                    let key = self.to_property_key(key_val);
                    self.set_property(obj, &key, val)?;
                    self.stack.push(val);
                }
                Op::SetElemTop => {
                    let key_val = self.pop()?;
                    let obj = self.pop()?;
                    let val = self.pop()?;
                    let key = self.to_property_key(key_val);
                    self.set_property(obj, &key, val)?;
                }
                Op::SetGetterObj => {
                    let key = constant_string(&constants, instr.operand as usize);
                    let fn_val = self.pop()?;
                    let obj = self.peek()?;
                    if let (ValueCase::Object(o_ref), ValueCase::Object(f_ref)) =
                        (obj.case(), fn_val.case())
                    {
                        let _ = f_ref;
                        if let Some(HeapObject::Ordinary {
                            getters,
                            has_accessors,
                            ..
                        }) = self.heap.get_mut(o_ref.0 as usize)
                        {
                            getters.insert(key.into_owned(), fn_val);
                            *has_accessors = 1;
                        }
                    }
                }
                Op::SetSetterObj => {
                    let key = constant_string(&constants, instr.operand as usize);
                    let fn_val = self.pop()?;
                    let obj = self.peek()?;
                    if let (ValueCase::Object(o_ref), ValueCase::Object(f_ref)) =
                        (obj.case(), fn_val.case())
                    {
                        let _ = f_ref;
                        if let Some(HeapObject::Ordinary {
                            setters,
                            has_accessors,
                            ..
                        }) = self.heap.get_mut(o_ref.0 as usize)
                        {
                            setters.insert(key.into_owned(), fn_val);
                            *has_accessors = 1;
                        }
                    }
                }
                Op::SetGetterComputedObj => {
                    let fn_val = self.pop()?;
                    let key_val = self.pop()?;
                    let key = self.to_property_key(key_val);
                    let obj = self.peek()?;
                    if let (ValueCase::Object(o_ref), ValueCase::Object(f_ref)) =
                        (obj.case(), fn_val.case())
                    {
                        let _ = f_ref;
                        if let Some(HeapObject::Ordinary {
                            getters,
                            has_accessors,
                            ..
                        }) = self.heap.get_mut(o_ref.0 as usize)
                        {
                            getters.insert(key, fn_val);
                            *has_accessors = 1;
                        }
                    }
                }
                Op::SetSetterComputedObj => {
                    let fn_val = self.pop()?;
                    let key_val = self.pop()?;
                    let key = self.to_property_key(key_val);
                    let obj = self.peek()?;
                    if let (ValueCase::Object(o_ref), ValueCase::Object(f_ref)) =
                        (obj.case(), fn_val.case())
                    {
                        let _ = f_ref;
                        if let Some(HeapObject::Ordinary {
                            setters,
                            has_accessors,
                            ..
                        }) = self.heap.get_mut(o_ref.0 as usize)
                        {
                            setters.insert(key, fn_val);
                            *has_accessors = 1;
                        }
                    }
                }
                Op::DelProp => {
                    let key = constant_string(&constants, instr.operand as usize);
                    let obj = self.pop()?;
                    if let Some(r) = obj.as_object() {
                        // 删除不改 shape：清槽 + 记入删除集 + 代数递增（见 delete_property）
                        self.delete_property(Value::Object(r), &key);
                    }
                    self.stack.push(Value::Boolean(true));
                }
                Op::DelElem => {
                    let key_val = self.pop()?;
                    let key = self.to_property_key(key_val);
                    let obj = self.pop()?;
                    if let Some(r) = obj.as_object() {
                        self.delete_property(Value::Object(r), &key);
                    }
                    self.stack.push(Value::Boolean(true));
                }

                // 12. 返回指令（return 穿越带 finally 的区域时先挂起、跑完 finally 再返回）
                Op::Return => {
                    let val = self.pop()?;
                    match self.exit_try(Completion::Return(val)) {
                        TryExitOutcome::Continue(next_pc) => {
                            pc = next_pc;
                            continue;
                        }
                        TryExitOutcome::Return(v) => return Ok(v),
                    }
                }
                Op::ReturnUndef => match self.exit_try(Completion::Return(Value::Undefined)) {
                    TryExitOutcome::Continue(next_pc) => {
                        pc = next_pc;
                        continue;
                    }
                    TryExitOutcome::Return(v) => return Ok(v),
                },

                // 13. ES6 类与面向对象指令
                Op::MakeClass => {
                    let class_idx = instr.operand as usize;
                    self.exec_make_class(class_idx)?;
                }
                Op::New => {
                    let num_args = instr.operand as usize;
                    let mut call_args = crate::call::CallArgs::with_capacity(num_args);
                    call_args.collect_from_stack(&mut self.stack, num_args)?;
                    let args = call_args.as_slice();
                    let callee = self.pop()?;
                    let res = self.do_construct(callee, args)?;
                    self.stack.push(res);
                }
                Op::ConstructThis => {
                    let num_args = instr.operand as usize;
                    let mut call_args = crate::call::CallArgs::with_capacity(num_args);
                    call_args.collect_from_stack(&mut self.stack, num_args)?;
                    let args = call_args.as_slice();
                    let callee = self.pop()?;
                    let res = self.do_construct_this(callee, args)?;
                    self.stack.push(res);
                }
                Op::CallThis => {
                    let num_args = instr.operand as usize;
                    let mut call_args = crate::call::CallArgs::with_capacity(num_args);
                    call_args.collect_from_stack(&mut self.stack, num_args)?;
                    let args = call_args.as_slice();
                    let callee = self.pop()?;
                    let this_val = *self.locals.first().unwrap_or(&Value::Undefined);
                    if let Some(c_ref) = callee.as_object() {
                        let (f_idx, uvs) =
                            if let Some(HeapObject::Closure {
                                func_idx, upvalues, ..
                            }) = self.heap.get(c_ref.0 as usize)
                            {
                                (Some(*func_idx), upvalues.clone())
                            } else if (c_ref.0 as usize) < self.module_functions.len() {
                                (Some(c_ref.0 as usize), Vec::new())
                            } else {
                                (None, Vec::new())
                            };
                        if let Some(fi) = f_idx {
                            let res = self.invoke_function(fi, this_val, args, uvs)?;
                            self.stack.push(res);
                        } else {
                            self.stack.push(Value::Undefined);
                        }
                    } else {
                        self.stack.push(Value::Undefined);
                    }
                }
                Op::GetProto => {
                    let obj = self.pop()?;
                    let proto = self.get_prototype(obj);
                    if let Some(p) = proto {
                        self.stack.push(Value::Object(p));
                    } else {
                        self.stack.push(Value::Null);
                    }
                }
                Op::Instanceof => {
                    let r = self.pop()?;
                    let l = self.pop()?;
                    let res = self.check_instanceof(l, r);
                    self.stack.push(Value::Boolean(res));
                }

                // 14. 异常与 try 语义（状态机移植自 Go 版 vm_exception.go）
                Op::TryEnter => {
                    let try_idx = instr.operand as usize;
                    let entry = self
                        .current_try_table
                        .get(try_idx)
                        .copied()
                        .ok_or(VmError::LocalOutOfRange)?;
                    self.try_stack.push(TryHandler {
                        try_idx,
                        entry,
                        exc: None,
                        phase: PHASE_TRY,
                        completion: None,
                    });
                }
                Op::TryExit => {
                    self.handle_try_exit(instr.operand as usize);
                }
                Op::TryExitFinally => match self.handle_try_exit_finally(instr.operand as usize) {
                    FinallyOutcome::Continue => {}
                    FinallyOutcome::ContinueAt(next_pc) => {
                        pc = next_pc;
                        continue;
                    }
                    FinallyOutcome::Rethrow(exc) => return Err(VmError::Thrown(exc)),
                    FinallyOutcome::Return(val) => return Ok(val),
                },
                Op::TryExitJmp => {
                    // break/continue 位于 try 区域内：跳转穿出区域前先运行 finally
                    let target = compute_jump_target(pc, instr.operand);
                    match self.exit_try(Completion::Jump(target)) {
                        TryExitOutcome::Continue(next_pc) => {
                            pc = next_pc;
                            continue;
                        }
                        // 不可达：Jump 完成动作永远解析为跳转而非 return
                        TryExitOutcome::Return(_) => return Ok(Value::Undefined),
                    }
                }
                Op::Throw => {
                    let exc = self.pop()?;
                    return Err(VmError::Thrown(exc));
                }

                // 15. 全局赋值与一元运算符
                Op::StoreGlobal => {
                    // 不带声明符的全局赋值写入全局变量表（对齐 Go 版 globalObj.Set）；
                    // CJS 注入名在模块函数帧内写入所属模块作用域（模块隔离）
                    let name = constant_string(&constants, instr.operand as usize);
                    let val = self.pop()?;
                    if crate::modules::CJS_INJECTED_NAMES.contains(&name.as_ref())
                        && let Some(si) = self.module_scope_of(self.current_func_idx)
                        && let Some(scope) = self.module_scopes.get_mut(si)
                    {
                        scope.vars.insert(name.into_owned(), val);
                    } else {
                        self.globals.insert(name.into_owned(), val);
                    }
                }
                Op::In => {
                    let r = self.pop()?;
                    let l = self.pop()?;
                    let key = self.to_property_key(l);
                    let res = self.has_property(r, &key);
                    self.stack.push(Value::Boolean(res));
                }
                Op::Typeof => {
                    let v = self.pop()?;
                    let s = self.typeof_value(v);
                    let r = self.alloc_string(s);
                    self.stack.push(Value::Object(r));
                }
                Op::TypeofGlobal => {
                    let name = constant_string(&constants, instr.operand as usize);
                    let v = self.resolve_global(&name);
                    let s = self.typeof_value(v);
                    let r = self.alloc_string(s);
                    self.stack.push(Value::Object(r));
                }

                // 16. 展开调用家族（f(...args) / obj.m(...args) / new X(...args) / super(...args)）
                Op::CallArgs => {
                    // 栈序 ... callee argsArray
                    let args_arr = self.pop()?;
                    let callee = self.pop()?;
                    let args = self.to_array_values(args_arr);
                    let ret = self.invoke_callable(callee, Value::Undefined, &args)?;
                    self.stack.push(ret);
                }
                Op::CallWithThisArgs => {
                    // 栈序 ... callee this argsArray
                    let args_arr = self.pop()?;
                    let this_val = self.pop()?;
                    let callee = self.pop()?;
                    let args = self.to_array_values(args_arr);
                    let ret = self.invoke_callable(callee, this_val, &args)?;
                    self.stack.push(ret);
                }
                Op::CallMethodArgs => {
                    // 栈序 ... receiver argsArray；操作数 = 方法名常量索引
                    let name = constant_string(&constants, instr.operand as usize);
                    let args_arr = self.pop()?;
                    let receiver = self.pop()?;
                    let args = self.to_array_values(args_arr);
                    let m_site = self.pic_site(pc);
                    let method = self.get_method_ic(receiver, &name, m_site)?;
                    let ret = self.invoke_callable(method, receiver, &args)?;
                    self.stack.push(ret);
                }
                Op::NewArgs => {
                    // 栈序 ... callee argsArray
                    let args_arr = self.pop()?;
                    let callee = self.pop()?;
                    let args = self.to_array_values(args_arr);
                    let res = self.do_construct(callee, &args)?;
                    self.stack.push(res);
                }
                Op::ConstructThisArgs => {
                    // super(...args)：参数表在栈顶，this 取当前帧 locals[0]
                    let args_arr = self.pop()?;
                    let callee = self.pop()?;
                    let args = self.to_array_values(args_arr);
                    let res = self.do_construct_this(callee, &args)?;
                    self.stack.push(res);
                }
                Op::SpreadObject => {
                    // { ...src }：把 src 自有属性逐个写入栈顶 dst（dst 不弹出）
                    let src = self.pop()?;
                    let dst = self.peek()?;
                    for (k, v) in self.own_properties(src) {
                        self.set_property(dst, &k, v)?;
                    }
                }
                Op::EnumKeys => {
                    // for-in 头部：快照原型链可枚举键为字符串数组（对齐 Go OpEnumKeys）
                    let src = self.pop()?;
                    // Proxy 对象：经 ownKeys trap 快照（trap 异常时按空集降级）
                    let keys: Vec<String> = if let Some(r) = src.as_object() {
                        if self.proxy_parts(r).is_some() {
                            self.proxy_own_keys(r).unwrap_or_default()
                        } else {
                            self.enumerate_for_in_keys(src)
                        }
                    } else {
                        self.enumerate_for_in_keys(src)
                    };
                    let key_refs: Vec<Value> = keys
                        .into_iter()
                        .map(|k| Value::Object(self.alloc_string(k)))
                        .collect();
                    let arr = self.alloc_array(key_refs);
                    self.stack.push(Value::Object(arr));
                }

                // 17. 生成器 / async 协程
                Op::Yield => {
                    // 挂起生成器帧：记录恢复点并以 Yielded 信号上抛（携带产出值）；
                    // 恢复时注入值压栈，成为 yield 表达式的求值结果
                    let produced = self.pop()?;
                    self.yield_pc = pc + 1;
                    return Err(VmError::Yielded(produced));
                }
                Op::Await => {
                    // await 是让出点：Node 语义下先把已排队的微任务跑完
                    self.drain_microtasks()?;
                    let awaited = self.pop()?;
                    let resolved = match awaited.case() {
                        ValueCase::Object(r) => match self.heap.get(r.0 as usize) {
                            Some(HeapObject::Promise {
                                pending: false,
                                value,
                                is_rejected,
                                ..
                            }) => {
                                if *is_rejected {
                                    // await 已拒绝的 promise：以拒绝原因在当前帧抛出
                                    // （帧内 try/catch 经正常异常路径接住）
                                    return Err(VmError::Thrown(*value));
                                }
                                Some(*value)
                            }
                            Some(HeapObject::Promise { pending: true, .. }) => {
                                // 真异步挂起：记录恢复点并以 Awaited 信号上抛，
                                // 由 async 驱动层捕获后挂起整帧（M2 事件循环模型）
                                self.yield_pc = pc + 1;
                                return Err(VmError::Awaited(r));
                            }
                            _ => Some(awaited),
                        },
                        _ => Some(awaited),
                    };
                    match resolved {
                        Some(v) => self.stack.push(v),
                        None => return Err(VmError::UnimplementedOpcode(instr.op)),
                    }
                }
                Op::GetIterator | Op::GetAsyncIterator => {
                    let val = self.pop()?;
                    if self.is_generator_obj(val)
                        || self.is_readable_obj(val)
                        || self.is_array_iterator(val)
                        || self.is_string_iterator(val)
                        || self.is_map_iterator(val)
                        || self.is_set_iterator(val)
                        || matches!(val.case(), ValueCase::Object(r) if self.has_own_slot(r.0 as usize, "_isReadable"))
                    {
                        // 生成器/流/四类内建迭代器对象自身即迭代器
                        // （JS 协议：iterator[Symbol.iterator]() === this）
                        self.stack.push(val);
                    } else if self.is_array_value(val) {
                        // 数组：物化下标迭代器（`for...of` / `for await...of` 共用）
                        if let Some(arr) = val.as_object() {
                            let it = self.alloc_array_iterator(arr);
                            self.stack.push(it);
                        } else {
                            self.stack.push(val);
                        }
                    } else if self.is_typed_array(val) {
                        // 类型化数组：物化元素快照迭代器（values 形态）
                        if let Some(ta) = val.as_object() {
                            let elems = self.ta_to_values(ta)?;
                            let snapshot = self.alloc_array(elems);
                            let it = self.alloc_array_iterator(snapshot);
                            self.stack.push(it);
                        } else {
                            self.stack.push(val);
                        }
                    } else if self.is_string_value(val) {
                        // 字符串：直接创建逐字符迭代器（避免 Symbol.iterator 查找开销）
                        if let Some(r) = val.as_object() {
                            let it = self.alloc_string_iterator(r);
                            self.stack.push(it);
                        } else {
                            self.stack.push(val);
                        }
                    } else if self.is_map_instance(val) {
                        // Map：entries 迭代器（产出 [key, value] 对）
                        if let Some(r) = val.as_object() {
                            let it = self.alloc_map_iterator(r, "entries");
                            self.stack.push(it);
                        } else {
                            self.stack.push(val);
                        }
                    } else if self.is_set_instance(val) {
                        // Set：values 迭代器（产出元素）
                        if let Some(r) = val.as_object() {
                            let it = self.alloc_set_iterator(r, "values");
                            self.stack.push(it);
                        } else {
                            self.stack.push(val);
                        }
                    } else {
                        // 自定义可迭代：读 Symbol.iterator 属性并调用取得迭代器
                        // （JS 协议：iterable[Symbol.iterator]() -> iterator）
                        let iter_sym = self.well_known_symbol("iterator");
                        let iter_ref = match iter_sym.case() {
                            ValueCase::Object(r) => r,
                            _ => unreachable!("well_known_symbol 返回符号对象"),
                        };
                        let key = crate::symbol::mangled_key(iter_ref);
                        let method = self.get_property(val, &key)?;
                        let is_closure = matches!(method.case(), ValueCase::Object(r)
                                if matches!(
                                    self.heap.get(r.0 as usize),
                                    Some(HeapObject::Closure { .. })
                                )
                        );
                        if is_closure {
                            let it = self.invoke_callable(method, val, &[])?;
                            self.stack.push(it);
                        } else {
                            let msg =
                                self.alloc_string("TypeError: value is not iterable".to_owned());
                            return Err(VmError::Thrown(Value::Object(msg)));
                        }
                    }
                }
                Op::MakeRegexp => {
                    // 正则字面量：弹 flags + pattern，构造 RegExp 对象（对齐 Go OpMakeRegexp）
                    let flags_val = self.pop()?;
                    let pattern_val = self.pop()?;
                    let regexp = HeapObject::RegExp {
                        pattern: self.format_value(pattern_val),
                        flags: self.to_property_key(flags_val),
                    };
                    let idx = self.push_object(regexp);
                    self.stack.push(Value::Object(idx));
                }

                // 其它高级对象与协程操作码（后续阶段扩展）
                Op::ForInNext | Op::CallThisArgs | Op::End => {
                    return Err(VmError::UnimplementedOpcode(instr.op));
                }
            }
            pc += 1;
        }
        Err(VmError::MissingReturn)
    }
}

/// 解释器静态常量键：有效字符串借用当前帧池，非法/非字符串保持旧回退。
/// 十进制 i128 按任意基数（2~36）格式化。
fn format_radix(n: i128, radix: u32) -> String {
    if radix == 10 || !(2..=36).contains(&radix) {
        return n.to_string();
    }
    if n == 0 {
        return "0".to_owned();
    }
    let neg = n < 0;
    let mut digits = Vec::new();
    let mut m = n.unsigned_abs();
    let _ = &mut digits;
    while m > 0 {
        let d = (m % radix as u128) as u32;
        digits.push(std::char::from_digit(d, radix).unwrap_or('0'));
        m /= radix as u128;
    }
    if neg {
        digits.push('-');
    }
    digits.iter().rev().collect()
}

impl Vm {
    /// SameValueZero 相等（`Array.prototype.includes` / Map/Set 键语义：
    /// NaN 视为相等、`+0`/`-0` 相等；对象按引用身份、**字符串按内容**）。
    ///
    /// 注意：`Value` 的 `PartialEq` 对 `Value::Object` 是**句柄比较**，而本 VM
    /// 以堆对象表示字符串——若直接 `a == b`，内容相同但句柄不同的两个字符串
    /// 会判为不等（`["a", "b"].includes("b")` 曾因此返回 `false`）。故委托
    /// [`crate::ops::strict_eq`]（`===` 语义，已按内容比对堆字符串）后补 NaN 自等。
    pub(crate) fn values_same_zero(&self, a: Value, b: Value) -> bool {
        if let (ValueCase::Number(x), ValueCase::Number(y)) = (a.case(), b.case()) {
            if x.is_nan() && y.is_nan() {
                return true;
            }
        }
        crate::ops::strict_eq(a, b, &self.heap, &self.current_constants)
    }
}

/// 归一化切片下标（`fill`/`copyWithin` 的 start/end 语义）：
/// 负值从尾部计数、NaN 视为 0、越界钳制到 `[0, len]`。
fn slice_index(n: Option<f64>, len: usize) -> usize {
    let n = n.unwrap_or(0.0);
    let n = if n.is_nan() || n.is_infinite() && n < 0.0 {
        0.0
    } else if n.is_infinite() {
        f64::INFINITY
    } else {
        n
    };
    let raw = if n < 0.0 { len as f64 + n } else { n };
    if raw.is_nan() || raw <= 0.0 {
        0
    } else if raw.is_infinite() || raw >= len as f64 {
        len
    } else {
        raw as usize
    }
}

/// 解析 `fill(value, start, end)` 的区间参数（第 2/3 实参）。
fn normalize_slice_range(args: &[Value], len: usize) -> (usize, usize) {
    let start = slice_index(args.get(1).map(|v| crate::ops::to_number(*v)), len);
    let end = match args.get(2) {
        Some(v) => slice_index(Some(crate::ops::to_number(*v)), len),
        None => len,
    };
    (start, end.max(start))
}

fn constant_string(constants: &std::rc::Rc<Vec<Constant>>, idx: usize) -> Cow<'_, str> {
    match constants.get(idx) {
        Some(Constant::String(value)) => Cow::Borrowed(value.as_str()),
        _ => Cow::Owned(format!("{idx}")),
    }
}

/// `Math.<method>(...)` 求值（单参数表 + 多参数 max/min/hypot/pow）。
fn math_method(method: &str, args: &[Value]) -> Value {
    let nums: Vec<f64> = args.iter().map(|v| to_number(*v)).collect();
    let value = match method {
        "abs" => nums.first().map(|n| n.abs()).unwrap_or(f64::NAN),
        "ceil" => nums.first().map(|n| n.ceil()).unwrap_or(f64::NAN),
        "floor" => nums.first().map(|n| n.floor()).unwrap_or(f64::NAN),
        "round" => nums
            .first()
            .map(|n| {
                // 规范 Math.round：非有限或整数原样返回；`-0` 与 `(-0.5, 0)` → `-0`；
                // 其余取最接近的整数，并列时取**较大**者（`-2.5 → -2`）。
                // 不能用 `(n + 0.5).floor()`：`0.49999999999999994 + 0.5` 恰好舍入成
                // `1.0` 而错得 `1`；`2^52` 附近的 `n + 0.5` 也会因舍入而多进一位。
                if !n.is_finite() || n.fract() == 0.0 {
                    return *n;
                }
                if *n > 0.0 && *n < 0.5 {
                    return 0.0;
                }
                if *n < 0.0 && *n >= -0.5 {
                    return -0.0;
                }
                let floor = n.floor();
                if *n - floor >= 0.5 {
                    floor + 1.0
                } else {
                    floor
                }
            })
            .unwrap_or(f64::NAN),
        "trunc" => nums.first().map(|n| n.trunc()).unwrap_or(f64::NAN),
        "sign" => match nums.first() {
            Some(n) if *n > 0.0 => 1.0,
            Some(n) if *n < 0.0 => -1.0,
            Some(n) if n.is_nan() => f64::NAN,
            _ => 0.0,
        },
        "sqrt" => nums.first().map(|n| n.sqrt()).unwrap_or(f64::NAN),
        "cbrt" => nums.first().map(|n| n.cbrt()).unwrap_or(f64::NAN),
        "pow" => {
            if nums.len() >= 2 {
                nums[0].powf(nums[1])
            } else {
                f64::NAN
            }
        }
        "max" => nums.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        "min" => nums.iter().cloned().fold(f64::INFINITY, f64::min),
        "hypot" => nums.iter().map(|n| n * n).sum::<f64>().sqrt(),
        "log" => nums.first().map(|n| n.ln()).unwrap_or(f64::NAN),
        "log2" => nums.first().map(|n| n.log2()).unwrap_or(f64::NAN),
        "log10" => nums.first().map(|n| n.log10()).unwrap_or(f64::NAN),
        "exp" => nums.first().map(|n| n.exp()).unwrap_or(f64::NAN),
        "random" => {
            use std::time::{SystemTime, UNIX_EPOCH};
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as f64)
                .unwrap_or(0.0);
            (nanos / 1_000_000_000.0).fract()
        }
        _ => f64::NAN,
    };
    Value::Number(value)
}

// RegExp 实例的 `lastIndex` 状态：`HeapObject::RegExp` 不携带可变字段，
// 以线程局部表承载（键为对象句柄，与 STREAM_STORE 同款模式）。
thread_local! {
    static REGEX_LAST_INDEX: std::cell::RefCell<Option<HashMap<u32, usize>>> =
        const { std::cell::RefCell::new(None) };
}

/// 读取 RegExp 实例的 `lastIndex`（未设置过按 0）。
pub(crate) fn regex_last_index(obj: u32) -> usize {
    REGEX_LAST_INDEX.with(|c| {
        c.borrow()
            .as_ref()
            .and_then(|m| m.get(&obj))
            .copied()
            .unwrap_or(0)
    })
}

/// 写入 RegExp 实例的 `lastIndex`。
pub(crate) fn set_regex_last_index(obj: u32, value: usize) {
    REGEX_LAST_INDEX.with(|c| {
        c.borrow_mut()
            .get_or_insert_with(HashMap::new)
            .insert(obj, value);
    });
}

/// `Object.prototype.hasOwnProperty(key)`：自有属性判定（receiver 经
/// `obj.hasOwnProperty(k)` 或 `.call(obj, k)` 泛型协议到达）。
fn objproto_has_own_property(vm: &mut Vm, args: &[Value]) -> Result<Value, crate::VmError> {
    let receiver = crate::builtins::current_receiver();
    let key = args
        .first()
        .map(|v| vm.format_value(*v))
        .unwrap_or_default();
    let is_own = match receiver.case() {
        ValueCase::Object(r) => match vm.heap.get(r.0 as usize) {
            Some(HeapObject::Array {
                elements,
                properties,
                ..
            }) => {
                key == "length"
                    || key
                        .parse::<usize>()
                        .map(|i| i < elements.len())
                        .unwrap_or(false)
                    || properties.contains_key(&key)
            }
            _ => vm.has_own_slot(r.0 as usize, &key),
        },
        _ => false,
    };
    Ok(Value::Boolean(is_own))
}

#[inline]
fn compute_jump_target(pc: usize, operand: u32) -> usize {
    let signed_off = if operand & 0x80_0000 != 0 {
        (operand | 0xFF00_0000) as i32
    } else {
        operand as i32
    };
    (((pc as i32 * 4) + 4 + signed_off) / 4) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_addition_and_returns_result() {
        let code = [
            Instr::new(Op::PushInt, 2),
            Instr::new(Op::PushInt, 3),
            Instr::new(Op::Add, 0),
            Instr::new(Op::Return, 0),
        ];
        let mut vm = Vm::new(0);
        match vm.run(&code) {
            Ok(v) => assert_eq!(v.as_number(), Some(5.0)),
            other => panic!("expected Number(5), got {other:?}"),
        }
    }

    #[test]
    fn round_trips_a_value_through_a_local_slot() {
        let code = [
            Instr::new(Op::PushInt, 41),
            Instr::new(Op::StoreLocal, 0),
            Instr::new(Op::LoadLocal, 0),
            Instr::new(Op::PushInt, 1),
            Instr::new(Op::Add, 0),
            Instr::new(Op::Return, 0),
        ];
        let mut vm = Vm::new(1);
        match vm.run(&code) {
            Ok(v) => assert_eq!(v.as_number(), Some(42.0)),
            other => panic!("expected Number(42), got {other:?}"),
        }
    }

    #[test]
    fn reports_underflow_instead_of_panicking() {
        let code = [Instr::new(Op::Add, 0), Instr::new(Op::Return, 0)];
        let mut vm = Vm::new(0);
        assert!(matches!(vm.run(&code), Err(VmError::StackUnderflow)));
    }

    #[test]
    fn reports_missing_return() {
        let code = [Instr::new(Op::PushInt, 1)];
        let mut vm = Vm::new(0);
        assert!(matches!(vm.run(&code), Err(VmError::MissingReturn)));
    }

    #[test]
    fn reports_local_out_of_range() {
        let code = [Instr::new(Op::LoadLocal, 5), Instr::new(Op::Return, 0)];
        let mut vm = Vm::new(1);
        assert!(matches!(vm.run(&code), Err(VmError::LocalOutOfRange)));
    }

    #[test]
    fn jit_switch_can_force_tier_zero_execution() {
        let mut vm = Vm::new(0);
        assert!(vm.jit_enabled());
        assert!(vm.set_jit_enabled(false));
        assert!(!vm.jit_enabled());
        assert!(!vm.set_jit_enabled(false));
        assert!(!vm.jit_enabled());
        assert!(!vm.set_jit_enabled(true));
        assert!(vm.jit_enabled());
    }

    #[test]
    fn fixed_argument_call_preserves_order_with_tier_zero() {
        let sum8 = FuncTemplate {
            name: "sum8".to_owned(),
            num_params: 8,
            num_locals: 9,
            is_var_args: false,
            is_generator: false,
            is_async: false,
            is_arrow: false,
            code: vec![
                Instr::new(Op::LoadLocal, 1),
                Instr::new(Op::LoadLocal, 2),
                Instr::new(Op::Add, 0),
                Instr::new(Op::LoadLocal, 3),
                Instr::new(Op::Add, 0),
                Instr::new(Op::LoadLocal, 4),
                Instr::new(Op::Add, 0),
                Instr::new(Op::LoadLocal, 5),
                Instr::new(Op::Add, 0),
                Instr::new(Op::LoadLocal, 6),
                Instr::new(Op::Add, 0),
                Instr::new(Op::LoadLocal, 7),
                Instr::new(Op::Add, 0),
                Instr::new(Op::LoadLocal, 8),
                Instr::new(Op::Add, 0),
                Instr::new(Op::Return, 0),
            ],
            max_stack: 16,
            source_file: String::new(),
            constants: Vec::new(),
            upvalues: Vec::new(),
            try_table: Vec::new(),
            line_table: Vec::new(),
        };
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
                Instr::new(Op::PushInt, 1),
                Instr::new(Op::PushInt, 2),
                Instr::new(Op::PushInt, 3),
                Instr::new(Op::PushInt, 4),
                Instr::new(Op::PushInt, 5),
                Instr::new(Op::PushInt, 6),
                Instr::new(Op::PushInt, 7),
                Instr::new(Op::PushInt, 8),
                Instr::new(Op::Call, 8),
                Instr::new(Op::Return, 0),
            ],
            max_stack: 16,
            source_file: String::new(),
            constants: vec![Constant::String("sum8".to_owned())],
            upvalues: Vec::new(),
            try_table: Vec::new(),
            line_table: Vec::new(),
        };
        let module = aluka_bytecode::BytecodeModule {
            header_extras: Vec::new(),
            version: 30,
            functions: vec![caller, sum8],
            classes: Vec::new(),
        };
        let mut vm = Vm::new(0);
        vm.load_module_for_test(&module);
        vm.set_jit_enabled(false);
        let callee = vm.alloc_closure(1);
        vm.globals.insert("sum8".to_owned(), Value::Object(callee));
        let result = vm
            .invoke_function(0, Value::Undefined, &[], Vec::new())
            .expect("Tier 0 调用");
        assert_eq!(result, Value::Number(36.0));
        assert!(vm.jit_counters.iter().all(|count| *count == 0));
    }

    #[test]
    fn call_args_uses_heap_path_at_nine_arguments() {
        let mut stack = (1..=9)
            .map(|number| Value::Number(f64::from(number)))
            .collect::<Vec<_>>();
        let mut args = crate::call::CallArgs::with_capacity(9);
        args.collect_from_stack(&mut stack, 9).expect("9 参数收集");
        assert!(stack.is_empty());
        assert_eq!(args.as_slice().len(), 9);
        assert_eq!(
            args.as_slice()
                .iter()
                .map(|value| value.as_number().unwrap_or(0.0))
                .collect::<Vec<_>>(),
            (1..=9).map(f64::from).collect::<Vec<_>>()
        );
    }

    #[test]
    fn native_function_call_still_reenters_vm_without_heap_clone() {
        let mut vm = Vm::new(0);
        let platform = vm.alloc_native_fn("os.arch");
        let result = vm
            .invoke_callable(Value::Object(platform), Value::Undefined, &[])
            .expect("调用 os.arch");
        let Some(r) = result.as_object() else {
            panic!("os.arch 应返回字符串对象，实际 {result:?}");
        };
        assert!(matches!(
            vm.heap.get(r.0 as usize),
            Some(HeapObject::String(_))
        ));
    }

    #[test]
    fn closure_metadata_reads_name_lazily_and_length_directly() {
        let func = FuncTemplate {
            name: "metadata".to_owned(),
            num_params: 3,
            num_locals: 4,
            is_var_args: false,
            is_generator: false,
            is_async: false,
            is_arrow: false,
            code: vec![Instr::new(Op::PushUndefined, 0), Instr::new(Op::Return, 0)],
            max_stack: 2,
            source_file: String::new(),
            constants: Vec::new(),
            upvalues: Vec::new(),
            try_table: Vec::new(),
            line_table: Vec::new(),
        };
        let module = aluka_bytecode::BytecodeModule {
            header_extras: Vec::new(),
            version: 30,
            functions: vec![func],
            classes: Vec::new(),
        };
        let mut vm = Vm::new(0);
        vm.load_module_for_test(&module);
        let closure = vm.alloc_closure(0);
        let length = vm
            .get_property(Value::Object(closure), "length")
            .expect("读取 length");
        assert_eq!(length, Value::Number(3.0));
        let name = vm
            .get_property(Value::Object(closure), "name")
            .expect("读取 name");
        let Some(name_ref) = name.as_object() else {
            panic!("name 应为字符串对象");
        };
        assert!(matches!(
            vm.heap.get(name_ref.0 as usize),
            Some(HeapObject::String(text)) if text == "metadata"
        ));
    }
}
