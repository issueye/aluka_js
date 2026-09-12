//! 分代标记-清除垃圾回收（ADR `docs/adr/0002-aluka-r-gc-selection.md` 原型 A
//! 在 VM 侧的落地：非移动、句柄=slab 下标、free-list 槽位粒度清扫）。
//!
//! # 结构
//!
//! [`GcState`] 是与 `Vm.heap` 平行的**侧表**（age/代别、free-list、统计），
//! 不改动 `HeapObject` 枚举本身——对象头的 age 字段以侧表等价实现（偏差
//! 记录：枚举 15+ 变体逐个加字段的侵入面远大于侧表，语义相同）。
//! 清扫把死对象槽位替换为 [`HeapObject::Free`] 占位，分配优先复用 free-list
//! 槽位——句柄永不变移，引用无需修补。
//!
//! # 根集（漏报根 = 悬垂，最重要的不变量）
//!
//! 1. **VM 结构图**：操作数栈、局部槽位、全局表、各类内置单例、微任务/
//!    宏任务队列、挂起 async 帧与生成器状态、try 栈挂起异常、模块导出缓存、
//!    内置注册表单例（见 [`Vm::collect_vm_roots`]）。
//! 2. **静态持有表**：内置库的 `static` 状态（解析器预存值、符号注册表、
//!    组合器状态、端口消息队列等）经各文件的 **root provider** 快照函数
//!    统一登记（见 [`static_roots`]，漏登记 = 悬垂）。
//!
//! # M6.1 形态：卡表写屏障 + 自适应堆伸缩 + 生产双代触发
//!
//! - **卡表（Card Table）**：写屏障 O(1) 登记脏卡（每 [`CARD_SIZE`] 个堆槽位
//!   一字节），minor 回收按脏卡收集「老写新」容器——替换原 `Vec<u32>` 记忆集
//!   （`contains` O(n) 去重、内存无界）；
//! - **自适应堆伸缩**：minor/major 触发阈值按上一轮**存活率**动态调整
//!   （存活高 → 抬高阈值摊薄回收成本；存活低 → 压低阈值及时释放内存），
//!   替换原固定 2000 万次分配阈值（实际运行从不触发）；
//! - **生产双代**：minor 与 major 均在 `push_object` 分配漏斗内自然触发
//!   （JIT 帧内除外，无栈映射期间回收不安全）；
//! - **压力验证模式**：`ALUKA_GC_STRESS=<N>` 每 N 次分配强制 major（N/4
//!   强制 minor）——漏登记的根/写屏障在该模式下确定性地表现为悬垂错误，
//!   全量套件 + conformance 在压力模式下跑绿 = 审计的机器验证。

use crate::heap::HeapObject;
use crate::interpreter::Vm;
use crate::value::{Upvalue, Value, ValueCase};
use aluka_core::ObjectRef;

/// GC 根集（VM 侧）。aluka-core 的 `RootSet` 服务于其自身槽位模型的
/// `core::Value`；VM 的运行时 `Value` 与之不同型，故本地定义。
/// **漏报根 = 悬垂**（ADR 0002 继承不变量）。
#[derive(Debug, Default)]
pub(crate) struct GcRoots(pub Vec<Value>);

impl GcRoots {
    pub(crate) fn push(&mut self, v: Value) {
        self.0.push(v);
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = Value> + '_ {
        self.0.iter().copied()
    }
}

/// minor 存活次数达到该值晋升老年代（ADR 原型 A 参数）。
pub(crate) const PROMOTE_AGE: u8 = 2;

/// 每张卡覆盖的堆槽位数（卡表粒度：8 字节卡 × 64 槽 ≈ 堆的 1/64 标记密度）。
pub(crate) const CARD_SIZE: u32 = 64;

/// 自适应阈值的下界（次回收至少间隔的分配数；过小 = 回收开销占比失控）。
pub(crate) const MINOR_TRIGGER_FLOOR: u32 = 4_096;
/// 自适应阈值的上界（防灾难性膨胀；约为 V8 新生代的对象数同量级）。
pub(crate) const MINOR_TRIGGER_CEIL: u32 = 1 << 20;
/// major 阈值相对 minor 阈值的倍率（全堆回收成本高，显著低频）。
pub(crate) const MAJOR_TRIGGER_RATIO: u32 = 8;

/// 出生水位线：对象出生后该数量的分配之内不可回收（原生 handler 构建
/// 窗口保护；取值 = minor 地板间隔，任何两次回收之间必然有这么多分配，
/// 而构建窗口必然短于一个回收间隔——除非 handler 自身分配超万级，那类
/// 场景应显式 gc_pinned）。
pub(crate) const BIRTH_WATERMARK: u32 = 4_096;

/// GC 侧表与统计。
#[derive(Debug)]
pub(crate) struct GcState {
    /// 与 `Vm.heap` 平行的对象年龄（分配序号对齐；Free 槽位无意义）
    pub(crate) ages: Vec<u8>,
    /// 槽位是否为清扫后的空闲占位
    pub(crate) is_free: Vec<bool>,
    /// 年轻代空闲槽位（清扫产生，分配复用）
    pub(crate) young_free: Vec<u32>,
    /// 老年代空闲槽位
    pub(crate) old_free: Vec<u32>,
    /// **卡表**：每 [`CARD_SIZE`] 个堆槽位一字节；写屏障置 1（脏卡 =
    /// 卡内可能存在「老写新」引用的容器），minor 回收消费后清零。
    pub(crate) cards: Vec<u8>,
    /// 槽位出生时的累计分配数（与 heap 平行）：出生水位线判据用。
    pub(crate) born: Vec<u64>,
    /// 出生水位线（分配数）；测试可调小以覆盖终审路径。
    pub(crate) birth_watermark: u32,
    /// minor 触发阈值（自适应：按上一轮存活率调整）
    pub(crate) minor_trigger: u32,
    /// major 触发阈值（自适应：minor_trigger × [`MAJOR_TRIGGER_RATIO`]）
    pub(crate) major_trigger: u32,
    /// 自上次 minor 回收以来的分配次数（minor 触发用）
    pub(crate) allocs_since_minor: u32,
    /// 自上次 major 回收以来的分配次数（major 触发用）
    pub(crate) allocs_since_major: u32,
    /// 累计分配对象数
    pub(crate) allocated: u64,
    /// 压力模式相位基准：`gc_stress_due` 以 `allocated - stress_base` 取模。
    /// 测试 `drain_gc` 将其对齐到当前分配数，使压力触发相位从 0 起算
    /// （避免因构建期分配数变化导致触发点落入被测代码的分配间隙）。
    pub(crate) stress_base: u64,
    /// 累计回收对象数
    pub(crate) reclaimed: u64,
    /// major 回收次数
    pub(crate) major_collections: u64,
    /// minor 回收次数
    pub(crate) minor_collections: u64,
}

impl Vm {
    /// 挂起回收（builtin 装配等「对象登记滞后于分配」的窗口；与 JIT 帧
    /// 内跳过回收同一不变量）。
    pub(crate) fn gc_suspend(&mut self) {
        self.gc_suspended += 1;
    }

    /// 恢复回收。
    pub(crate) fn gc_resume(&mut self) {
        self.gc_suspended = self.gc_suspended.saturating_sub(1);
    }
}

impl Default for GcState {
    fn default() -> Self {
        GcState {
            ages: Vec::new(),
            is_free: Vec::new(),
            young_free: Vec::new(),
            old_free: Vec::new(),
            cards: Vec::new(),
            born: Vec::new(),
            birth_watermark: BIRTH_WATERMARK,
            minor_trigger: MINOR_TRIGGER_FLOOR,
            major_trigger: MINOR_TRIGGER_FLOOR * MAJOR_TRIGGER_RATIO,
            allocs_since_minor: 0,
            allocs_since_major: 0,
            allocated: 0,
            stress_base: 0,
            reclaimed: 0,
            major_collections: 0,
            minor_collections: 0,
        }
    }
}

impl GcState {
    /// 分配记账；返回 (达到 minor 阈值, 达到 major 阈值)。
    /// 阈值为自适应值（见 [`Self::adapt_after_minor`]）。
    pub(crate) fn on_alloc(&mut self) -> (bool, bool) {
        self.allocated += 1;
        self.allocs_since_minor += 1;
        self.allocs_since_major += 1;
        let minor = self.allocs_since_minor >= self.minor_trigger;
        let major = self.allocs_since_major >= self.major_trigger;
        (minor, major)
    }

    /// 卡表登记（写屏障调用；槽位所在卡置脏）。O(1)，无去重开销。
    pub(crate) fn mark_card(&mut self, slot: u32) {
        let card = (slot / CARD_SIZE) as usize;
        if card >= self.cards.len() {
            self.cards.resize(card + 1, 0);
        }
        self.cards[card] = 1;
    }

    /// minor 回收后的自适应调整（Hertz 启发式的存活率形态）：
    /// 上一轮年轻代存活高（多数对象长寿）→ 抬阈值摊薄 minor 成本；
    /// 存活低（朝生暮死）→ 压阈值及时释放。major 阈值随动保持倍率。
    pub(crate) fn adapt_after_minor(&mut self, survivors: u32, reclaimed_minor: u32) {
        let total = survivors + reclaimed_minor;
        if total == 0 {
            return;
        }
        // 存活率 → 目标间隔：全存活（1.0）= 16×地板；全死亡（0.0）= 地板
        let survival = survivors as f64 / total as f64;
        let target = MINOR_TRIGGER_FLOOR as f64 * (1.0 + 15.0 * survival);
        self.minor_trigger = (target as u32).clamp(MINOR_TRIGGER_FLOOR, MINOR_TRIGGER_CEIL);
        self.major_trigger = self
            .minor_trigger
            .saturating_mul(MAJOR_TRIGGER_RATIO)
            .clamp(MINOR_TRIGGER_FLOOR, MINOR_TRIGGER_CEIL << 4);
    }

    /// 出生水位线判据：出生后 [`BIRTH_WATERMARK`] 次分配内的对象不可回收。
    ///
    /// 覆盖「原生 handler 跨分配构建对象」的系统性模式——createHash/
    /// createServer 等在 Rust 局部变量持有新对象、逐步挂属性/派生状态期间
    /// 遭遇回收：对象尚无 VM 侧根，水位线保证其存活过整个构建窗口
    /// （crypto 实例在 stress≤8 实测暴露；单周期宽限期不够——构建可能
    /// 跨两次回收）。
    pub(crate) fn birth_protected(&self, idx: usize) -> bool {
        // 严格小于：birth_watermark = 0 即完全关闭保护（测试与强制口径）
        self.allocated - self.born.get(idx).copied().unwrap_or(0) < self.birth_watermark as u64
    }
}

/// 压力验证模式间隔（`ALUKA_GC_STRESS=<N>`；0/未设 = 关闭）。
///
/// 每 N 次分配强制一次 major + minor 回收：把「漏登记根 / 漏写屏障」从
/// 随机潜伏变成确定性错误。全量测试套件 + conformance 在该模式下跑绿，
/// 即变异点审计的机器验证（M6.1 审计口径）。
pub(crate) fn gc_stress_interval() -> u32 {
    use std::sync::OnceLock;
    static CELL: OnceLock<u32> = OnceLock::new();
    *CELL.get_or_init(|| {
        std::env::var("ALUKA_GC_STRESS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(0)
    })
}

/// 给定累计分配数，当前是否到了强制回收点。
pub(crate) fn gc_stress_due(allocated: u64, stress_base: u64) -> bool {
    let n = gc_stress_interval();
    n > 0 && allocated.wrapping_sub(stress_base) % (n as u64) == 0
}

/// 压力模式收集器选择（诊断用）：`major` 只跑 major，`minor` 只跑 minor。
pub(crate) fn gc_stress_mode() -> &'static str {
    use std::sync::OnceLock;
    static CELL: OnceLock<&'static str> = OnceLock::new();
    CELL.get_or_init(|| {
        std::env::var("ALUKA_GC_MODE")
            .ok()
            .filter(|v| v == "major" || v == "minor")
            .map(|v| match v.as_str() {
                "major" => "major",
                _ => "minor",
            })
            .unwrap_or("both")
    })
}

/// 嵌套执行期间被换出的外层帧状态（GC 根集合成员）。
///
/// 解释器在嵌套调用（`invoke_function` / `run_func` / JIT 热点入口 / 生成器
/// 与 async 帧驱动）时，把外层帧的 locals/upvalues/try 栈从 `Vm` 字段换出；
/// 这些状态在换出期间若只存于 Rust 栈变量，就逃过了根扫描，嵌套执行触发
/// GC 时会把它们引用的对象误判不可达并回收——堆格子复用后恢复的帧即读到
/// 悬垂引用（M2.4 express 依赖树加载实测：http-errors 帧 slot 7 的闭包被
/// 回收复用为 depd 的 eehaslisteners）。换出状态统一登记进本栈，由
/// `collect_vm_roots` 保守按根扫描（多留不死，杜绝悬垂）。
#[derive(Default)]
pub(crate) struct SavedFrameState {
    /// 外层帧局部槽位
    pub(crate) locals: Vec<Value>,
    /// 外层帧上值（`Rc` 共享句柄，其单元格内容亦是根）
    pub(crate) upvalues: Vec<Upvalue>,
    /// 外层帧活跃的打开上值表
    pub(crate) open_upvalues: Vec<(usize, Upvalue)>,
    /// 外层帧 try handler 栈（挂起异常/完成值亦是根）
    pub(crate) try_stack: Vec<crate::exception::TryHandler>,
}

impl Vm {
    /// 对象当前年龄。
    pub(crate) fn gc_age(&self, r: ObjectRef) -> Option<u8> {
        let idx = r.0 as usize;
        if idx < self.gc.ages.len() && !self.gc.is_free[idx] {
            self.gc.ages.get(idx).copied()
        } else {
            None
        }
    }

    /// 是否为老年代对象（年龄达到晋升阈值）。
    pub(crate) fn gc_is_old(&self, r: ObjectRef) -> bool {
        self.gc_age(r).is_some_and(|a| a >= PROMOTE_AGE)
    }

    /// 构建完整根集：VM 结构图 + 静态持有表快照。
    pub(crate) fn build_gc_roots(&self) -> GcRoots {
        let mut roots = GcRoots::default();
        self.collect_vm_roots(&mut roots);
        static_roots(&mut roots);
        roots
    }

    /// VM 结构图根源登记（漏一项 = 一类悬垂）。
    pub(crate) fn collect_vm_roots(&self, out: &mut GcRoots) {
        // 操作数栈与局部槽位
        for v in &self.stack {
            out.push(*v);
        }
        for v in &self.locals {
            out.push(*v);
        }
        // 全局变量表
        for v in self.globals.values() {
            out.push(*v);
        }
        // 内置单例
        for r in [
            self.object_prototype,
            self.array_prototype,
            self.math_object,
            self.str_proto,
            self.bool_proto,
            self.num_proto,
            self.fn_proto,
            self.regexp_proto,
            self.array_proto_surface,
            self.container_proto,
            self.symbol_proto,
            self.date_proto,
            self.function_ctor,
            self.error_ctor,
            self.error_prototype,
            self.array_ctor,
            self.object_ctor,
            self.promise_ctor,
            self.map_ctor,
            self.set_ctor,
            self.regexp_ctor,
            self.regexp_prototype,
            self.proxy_ctor,
            self.reflect_object,
            self.process_object,
            self.path_module,
            self.os_module,
            self.stream_module,
            self.events_module,
        ]
        .into_iter()
        .flatten()
        {
            out.push(Value::Object(r));
        }
        // 上值（当前帧 + 打开上值表）：内层 RefCell 解引用
        for uv in &self.current_upvalues {
            out.push(*uv.0.borrow());
        }
        for uv in self.open_upvalues.values() {
            out.push(*uv.0.borrow());
        }
        // 微任务队列：nextTick 回调 + Promise 回调 + 挂起帧恢复
        for cb in &self.nexttick_queue {
            out.push(*cb);
        }
        for job in &self.microtask_queue {
            match job {
                crate::builtins::Job::Call(cb, arg) => {
                    out.push(*cb);
                    out.push(*arg);
                }
                crate::builtins::Job::ResumeFrame(r)
                | crate::builtins::Job::ResumeFrameRejected(r) => {
                    self.push_resume_roots(out, r);
                }
                crate::builtins::Job::Reaction {
                    cb,
                    arg,
                    resolver,
                    reject_resolver,
                    ..
                } => {
                    out.push(*cb);
                    out.push(*arg);
                    out.push(*resolver);
                    out.push(*reject_resolver);
                }
                crate::builtins::Job::ResolveLater { resolver, arg }
                | crate::builtins::Job::RejectLater { resolver, arg } => {
                    out.push(*resolver);
                    out.push(*arg);
                }
            }
        }
        // 宏任务（定时器回调）
        for (_, _, _, cb, _) in &self.macro_tasks {
            out.push(*cb);
        }
        // try 栈：挂起异常与挂起 return
        for h in &self.try_stack {
            if let Some(exc) = h.exc {
                out.push(exc);
            }
            if let Some(crate::exception::Completion::Return(v)) = h.completion {
                out.push(v);
            }
        }
        // 生成器状态
        for g in self.generators.values() {
            out.push(g.this_val);
            for a in &g.args {
                out.push(*a);
            }
            for uv in &g.upvalues {
                out.push(*uv.0.borrow());
            }
            if let Some(frame) = &g.frame {
                for v in &frame.stack {
                    out.push(*v);
                }
                for v in &frame.locals {
                    out.push(*v);
                }
                for uv in &frame.upvalues {
                    out.push(*uv.0.borrow());
                }
                for uv in frame.open_upvalues.values() {
                    out.push(*uv.0.borrow());
                }
                for h in &frame.try_stack {
                    if let Some(exc) = h.exc {
                        out.push(exc);
                    }
                    if let Some(crate::exception::Completion::Return(v)) = h.completion {
                        out.push(v);
                    }
                }
            }
        }
        // 挂起 async 帧恢复登记
        for r in self.promise_resumes.values() {
            self.push_resume_roots(out, r);
        }
        // 嵌套执行换出的外层帧（保存帧寄存器：invoke_function/run_func/
        // CallerFrame/jit 入口换出期间同样是活跃根，漏扫 = 悬垂复用）
        for f in &self.gc_saved_frames {
            for v in &f.locals {
                out.push(*v);
            }
            for uv in &f.upvalues {
                out.push(*uv.0.borrow());
            }
            for (_, uv) in &f.open_upvalues {
                out.push(*uv.0.borrow());
            }
            for h in &f.try_stack {
                if let Some(exc) = h.exc {
                    out.push(exc);
                }
                if let Some(crate::exception::Completion::Return(v)) = h.completion {
                    out.push(v);
                }
            }
        }
        // 原型面构造器缓存（String/Boolean/Number/... NativeCtor 单例）
        for c in self.ctor_cache.values() {
            out.push(Value::Object(*c));
        }
        // M6.1 根审计补漏：单例字段（fs 对象 / require 入口 / env 对象 /
        // objproto hasOwnProperty 分派项 / 入口异步完成 Promise /
        // 模块 require 实例键）
        if let Some(r) = self.fs_object {
            out.push(Value::Object(r));
        }
        if let Some(r) = self.require_fn {
            out.push(Value::Object(r));
        }
        if let Some(r) = self.env_object {
            out.push(Value::Object(r));
        }
        if let Some(r) = self.objproto_has_own {
            out.push(Value::Object(r));
        }
        if let Some(v) = self.last_entry_async_promise {
            out.push(v);
        }
        for r in self.require_bases.keys() {
            out.push(Value::Object(*r));
        }
        // CJS 模块作用域表：注入名值（module/exports/require 等）是活跃根
        for scope in &self.module_scopes {
            for v in scope.vars.values() {
                out.push(*v);
            }
        }
        // 模块导出缓存与内置注册表单例
        for v in self.module_exports.values() {
            out.push(*v);
        }
        // 钉扎句柄（require 进行中的 module 对象；M2.4 排障补钉）
        for &h in &self.gc_pinned {
            out.push(Value::Object(aluka_core::ObjectRef(h)));
        }
        for r in self.builtin_registry.module_handles() {
            out.push(Value::Object(r));
        }
    }

    /// 挂起帧恢复登记的根源（async/await 中途态）。
    fn push_resume_roots(&self, out: &mut GcRoots, r: &crate::builtins::PendingResume) {
        out.push(Value::Object(r.promise));
        out.push(Value::Object(r.awaited));
        for v in &r.frame.stack {
            out.push(*v);
        }
        for v in &r.frame.locals {
            out.push(*v);
        }
        for uv in &r.frame.upvalues {
            out.push(*uv.0.borrow());
        }
        for uv in r.frame.open_upvalues.values() {
            out.push(*uv.0.borrow());
        }
        for h in &r.frame.try_stack {
            if let Some(exc) = h.exc {
                out.push(exc);
            }
            if let Some(crate::exception::Completion::Return(v)) = h.completion {
                out.push(v);
            }
        }
    }

    /// 执行一次 major 全堆标记-清除。返回回收对象数。
    pub(crate) fn collect_major_gc(&mut self) -> u64 {
        let roots = self.build_gc_roots();
        let mut marked = vec![false; self.heap.len()];
        for root in roots.iter() {
            if let Some(r) = root.as_object() {
                self.mark_all(r.0, &mut marked);
            }
        }
        let mut reclaimed = 0u64;
        for (idx, slot) in self.heap.iter_mut().enumerate() {
            if marked[idx] || self.gc.is_free[idx] {
                continue;
            }
            if self.gc.birth_protected(idx) {
                continue; // 出生水位线内：构建窗口保护
            }
            // 死对象：替换为 Free 占位，槽位按当前代别归入对应 free-list
            let is_old = self.gc.ages.get(idx).copied().unwrap_or(0) >= PROMOTE_AGE;
            if is_old {
                self.gc.old_free.push(idx as u32);
            } else {
                self.gc.young_free.push(idx as u32);
            }
            *slot = HeapObject::Free;
            self.gc.is_free[idx] = true;
            reclaimed += 1;
        }
        // 存活对象晋升（major 后一律视为老年代——ADR 原型 A 语义）
        for (idx, age) in self.gc.ages.iter_mut().enumerate() {
            if !self.gc.is_free[idx]
                && *age < PROMOTE_AGE
                && marked.get(idx).copied().unwrap_or(false)
            {
                *age = PROMOTE_AGE;
            }
        }
        self.gc.major_collections += 1;
        self.gc.reclaimed += reclaimed;
        self.gc.allocs_since_major = 0;
        self.gc.allocs_since_minor = 0;
        // major 全堆回收后卡表整体失效（全部引用已重新标记过）
        self.gc.cards.clear();
        reclaimed
    }

    /// 执行一次 minor 回收（只清年轻代；脏卡容器内的老→新引用作次级根）。
    pub(crate) fn collect_minor_gc(&mut self) -> u64 {
        let roots = self.build_gc_roots();
        let mut marked = vec![false; self.heap.len()];
        for root in roots.iter() {
            if let Some(r) = root.as_object() {
                self.mark_young(r.0, &mut marked);
            }
        }
        // 脏卡 = 写屏障登记过「可能含老写新」的槽位区间：逐卡扫描其中的
        // 非空闲老年代对象，其年轻引用按次级根标记（语义与记忆集等价，
        // 粒度放宽到卡——卡内无年轻引用的对象只是多扫描一次 trace_refs）。
        let cards = std::mem::take(&mut self.gc.cards);
        let mut still_dirty: Vec<u32> = Vec::new();
        for (card, dirty) in cards.iter().enumerate() {
            if *dirty == 0 {
                continue;
            }
            let start = card as u32 * CARD_SIZE;
            let end = (start + CARD_SIZE).min(self.heap.len() as u32);
            for old_idx in start..end {
                if self
                    .gc
                    .is_free
                    .get(old_idx as usize)
                    .copied()
                    .unwrap_or(true)
                {
                    continue;
                }
                if self
                    .gc
                    .ages
                    .get(old_idx as usize)
                    .copied()
                    .unwrap_or(PROMOTE_AGE)
                    < PROMOTE_AGE
                {
                    continue; // 年轻容器自身不构成次级根
                }
                let Some(obj) = self.heap.get(old_idx as usize) else {
                    continue;
                };
                let mut still = false;
                obj.trace_refs(|t| {
                    if !self.gc.is_free.get(t as usize).copied().unwrap_or(true)
                        && self.gc.ages.get(t as usize).copied().unwrap_or(PROMOTE_AGE)
                            < PROMOTE_AGE
                    {
                        still = true;
                    }
                });
                if !still {
                    continue;
                }
                still_dirty.push(old_idx);
                if let Some(obj) = self.heap.get(old_idx as usize) {
                    obj.trace_refs(|t| {
                        if !self.gc.is_free.get(t as usize).copied().unwrap_or(true)
                            && self.gc.ages.get(t as usize).copied().unwrap_or(PROMOTE_AGE)
                                < PROMOTE_AGE
                        {
                            self.mark_young(t, &mut marked);
                        }
                    });
                }
            }
        }
        let mut survivors = 0u32;
        let mut reclaimed = 0u64;
        for (idx, slot) in self.heap.iter_mut().enumerate() {
            if self.gc.is_free[idx] {
                continue;
            }
            let is_young = self.gc.ages.get(idx).copied().unwrap_or(PROMOTE_AGE) < PROMOTE_AGE;
            if !is_young {
                continue; // 老年代不参与 minor
            }
            if marked[idx] {
                survivors += 1;
                let promoted = {
                    let age = self.gc.ages.get_mut(idx).expect("ages 与 heap 平行");
                    *age += 1;
                    *age >= PROMOTE_AGE
                };
                if promoted {
                    // 晋升守卫：升代对象可能持有年轻引用，而其卡从未因写入
                    // 置脏（引用建立时它自己还是年轻代）——不置卡则下一个
                    // minor 会把这些年轻引用误判为垃圾（perf_hooks 实测暴露）
                    self.gc.mark_card(idx as u32);
                }
                continue;
            }
            if self.gc.birth_protected(idx) {
                continue; // 出生水位线内：构建窗口保护
            }
            // 死对象：替换为 Free 占位（年轻代槽位入年轻 free-list）
            self.gc.young_free.push(idx as u32);
            *slot = HeapObject::Free;
            self.gc.is_free[idx] = true;
            reclaimed += 1;
        }
        // 仍持年轻引用的脏卡容器：重新登记（写屏障只在写入时置卡，
        // 存活期跨回收的引用必须显式重卡）
        for old_idx in still_dirty {
            self.gc.mark_card(old_idx);
        }
        self.gc.minor_collections += 1;
        self.gc.reclaimed += reclaimed;
        self.gc.allocs_since_minor = 0;
        self.gc.adapt_after_minor(survivors, reclaimed as u32);
        reclaimed
    }

    /// minor 回收开关（测试用；生产路径保持 major-only）。
    /// minor 机制当前仅测试路径使用（生产 major-only，见模块文档偏差记录）。
    #[allow(dead_code)]
    /// 写屏障：老年代容器写入年轻代引用时置脏卡（minor 回收的次级根来源）。
    /// **全部「老写新」变异点必须调用**（set_property / 数组元素 / Map.set /
    /// 事件监听 / Readable 缓冲 / Promise 处理器 / upvalue 写入），漏调用 =
    /// minor 悬垂。卡表 O(1) 登记无去重开销；压力模式（ALUKA_GC_STRESS）
    /// 下漏调用会确定性暴露为悬垂复用错误。
    pub(crate) fn gc_write_barrier(&mut self, container: ObjectRef, val: Value) {
        if !self.gc_is_old(container) {
            return;
        }
        let young_target = match val.case() {
            ValueCase::Object(r) => {
                !self.gc.is_free.get(r.0 as usize).copied().unwrap_or(true)
                    && self
                        .gc
                        .ages
                        .get(r.0 as usize)
                        .copied()
                        .unwrap_or(PROMOTE_AGE)
                        < PROMOTE_AGE
            }
            _ => false,
        };
        if young_target {
            self.gc.mark_card(container.0);
        }
    }

    /// 强制执行一次 major 回收（测试与手动触发入口）。
    pub fn force_gc(&mut self) -> u64 {
        self.collect_major_gc()
    }

    /// GC 统计快照：(累计分配, 累计回收, major 次数, minor 次数)。
    pub fn gc_stats(&self) -> (u64, u64, u64, u64) {
        (
            self.gc.allocated,
            self.gc.reclaimed,
            self.gc.major_collections,
            self.gc.minor_collections,
        )
    }

    /// 从 `root` 出发标记全堆可达对象（major）。
    fn mark_all(&self, idx: u32, marked: &mut [bool]) {
        let mut stack = vec![idx];
        while let Some(i) = stack.pop() {
            let i = i as usize;
            if i >= self.heap.len() || marked[i] || self.gc.is_free[i] {
                continue;
            }
            let Some(obj) = self.heap.get(i) else {
                continue;
            };
            marked[i] = true;
            obj.trace_refs(|target| stack.push(target));
        }
    }

    /// 从 `root` 出发标记年轻代可达对象（minor：越过老年代）。
    fn mark_young(&self, idx: u32, marked: &mut [bool]) {
        let mut stack = vec![idx];
        while let Some(i) = stack.pop() {
            let i = i as usize;
            if i >= self.heap.len() || marked[i] || self.gc.is_free[i] {
                continue;
            }
            if self.gc.ages.get(i).copied().unwrap_or(PROMOTE_AGE) >= PROMOTE_AGE {
                continue; // 老年代对象：越过（经记忆集单独作根）
            }
            let Some(obj) = self.heap.get(i) else {
                continue;
            };
            marked[i] = true;
            obj.trace_refs(|target| stack.push(target));
        }
    }
}

/// 静态持有表根源快照：集中登记各内置库的 root provider。
/// **新增持有 `Value` 的静态必须在此登记 provider**（漏登记 = 悬垂）。
fn static_roots(out: &mut GcRoots) {
    crate::builtins::promise::combiner_roots(out);
    crate::builtins::promise::reaction_roots(out);
    crate::builtins::timers::resolver_roots(out);
    crate::symbol::registry_roots(out);
    // M4：Node/Web 流静态状态表（缓冲 chunk、监听器、互通桥）
    crate::builtins::stream::store_roots(out);
    crate::builtins::stream_web::store_roots(out);
    // M6.1 根审计补全：其余持有堆值的线程局部静态表（漏一项 = 悬垂）
    crate::builtins::dispatch_tls_roots(out);
    crate::builtins::child_process::proc_common::store_roots(out);
    crate::builtins::events::store_roots(out);
    crate::builtins::net::store_roots(out);
    crate::builtins::dgram::store_roots(out);
    crate::builtins::worker_threads::store_roots(out);
    crate::builtins::http::state::store_roots(out);
    crate::builtins::http2::store_roots(out);
    crate::builtins::zlib::store_roots(out);
    crate::builtins::async_hooks::store_roots(out);
    crate::builtins::domain::store_roots(out);
    crate::builtins::diagnostics_channel::store_roots(out);
    crate::builtins::readline::store_roots(out);
    crate::builtins::sqlite::store_roots(out);
    crate::builtins::test::registry::store_roots(out);
    crate::builtins::test::mock::store_roots(out);
    crate::builtins::crypto::async_cb_roots(out);
    crate::builtins::readline_promises::store_roots(out);
    crate::builtins::test::state::store_roots(out);
    crate::builtins::test::store_roots(out);
    crate::builtins::vm::store_roots(out);
    crate::builtins::module::store_roots(out);
    crate::builtins::cluster::store_roots(out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::HeapObject;

    #[test]
    fn promote_age_constant_matches_adr() {
        assert_eq!(PROMOTE_AGE, 2, "ADR 原型 A：存活 2 次 minor 晋升");
    }

    /// 强制两次回收：第一轮移入待定，第二轮终审落 Free（两遍惰性清扫闭环）。
    /// 排水式预热：关闭水位线连扫两轮，放行全部启动期残留；
    /// 之后被测分配的回收计数即为净口径。
    fn drain_gc(vm: &mut Vm) {
        vm.gc.birth_watermark = 0;
        vm.force_gc();
        vm.force_gc();
        // 压力模式相位归零：被测分配从 0 起算，触发点确定
        vm.gc.stress_base = vm.gc.allocated;
    }

    /// 无引用的分配在两遍回收后被终审释放（槽位转 Free 并入 free-list）。
    #[test]
    fn unreferenced_object_is_reclaimed() {
        let mut vm = Vm::new(0);
        drain_gc(&mut vm); // 排水预热
        let dead = vm.alloc_ordinary();
        assert!(!vm.gc.is_free[dead.0 as usize]);
        let reclaimed = vm.force_gc();
        assert_eq!(reclaimed, 1, "无引用对象应在回收中被释放");
        assert!(vm.gc.is_free[dead.0 as usize]);
        assert!(matches!(vm.heap[dead.0 as usize], HeapObject::Free));
    }

    /// 全局表持有的对象跨回收存活（VM 结构图根）。
    #[test]
    fn globally_reachable_object_survives() {
        let mut vm = Vm::new(0);
        let keep = vm.alloc_ordinary();
        vm.globals.insert("k".to_owned(), Value::Object(keep));
        vm.force_gc();
        assert!(!vm.gc.is_free[keep.0 as usize], "全局表根必须保活");
    }

    /// 对象图：根可达链上的对象全部存活，链外垃圾回收。
    #[test]
    fn graph_trace_keeps_reachable_chain() {
        let mut vm = Vm::new(0);
        drain_gc(&mut vm); // 排水预热
        let root = vm.alloc_ordinary();
        let mid = vm.alloc_ordinary();
        let dead = vm.alloc_ordinary();
        let _ = vm.set_property(Value::Object(root), "mid", Value::Object(mid));
        let tag = vm.alloc_string("v".to_owned());
        let _ = vm.set_property(Value::Object(mid), "tag", Value::Object(tag));
        vm.globals.insert("r".to_owned(), Value::Object(root));
        let reclaimed = vm.force_gc();
        assert_eq!(reclaimed, 1, "链外对象应被回收");
        assert!(!vm.gc.is_free[root.0 as usize]);
        assert!(!vm.gc.is_free[mid.0 as usize]);
        assert!(vm.gc.is_free[dead.0 as usize], "链外对象应被回收");
    }

    /// 出生水位线：新生对象在其后 BIRTH_WATERMARK 次分配内不可回收
    /// （原生 handler 构建窗口保护；crypto/流实例 stress 实测暴露）。
    #[test]
    fn birth_watermark_protects_new_objects() {
        let mut vm = Vm::new(0);
        drain_gc(&mut vm); // 排水预热（恢复水位线，验证新生儿保护）
        vm.gc.birth_watermark = BIRTH_WATERMARK;
        let obj = vm.alloc_ordinary();
        vm.globals.insert("k".to_owned(), Value::Object(obj)); // 根登记
        // 出生即遭遇强制回收（水位线内）：必须存活且对象体完整
        vm.collect_major_gc();
        vm.collect_minor_gc();
        assert!(!vm.gc.is_free[obj.0 as usize], "水位线内对象不可回收");
        assert!(matches!(
            vm.heap[obj.0 as usize],
            HeapObject::Ordinary { .. }
        ));
        // 老对象可正常回收（水位线只保护新生儿）
        vm.force_gc(); // obj 晋升老年代
        assert!(vm.gc_is_old(obj));
    }

    /// 循环结构无外部根时被回收（追踪式 GC 的核心价值）。
    #[test]
    fn cyclic_garbage_is_collected() {
        let mut vm = Vm::new(0);
        drain_gc(&mut vm); // 排水预热
        let a = vm.alloc_ordinary();
        let b = vm.alloc_ordinary();
        let _ = vm.set_property(Value::Object(a), "b", Value::Object(b));
        let _ = vm.set_property(Value::Object(b), "a", Value::Object(a));
        let reclaimed = vm.force_gc();
        assert_eq!(reclaimed, 2, "环上两个垃圾都应回收");
        assert!(vm.gc.is_free[a.0 as usize] && vm.gc.is_free[b.0 as usize]);
    }

    /// major 存活对象晋升老年代（age 达到 PROMOTE_AGE）。
    #[test]
    fn major_survivors_promote() {
        let mut vm = Vm::new(0);
        let keep = vm.alloc_ordinary();
        vm.globals.insert("k".to_owned(), Value::Object(keep));
        vm.force_gc();
        assert!(vm.gc_is_old(keep), "major 存活应晋升老年代");
    }

    /// 空闲槽位被后续分配复用（句柄稳定、age 重置为年轻代）。
    #[test]
    fn free_slot_is_reused_with_reset_age() {
        let mut vm = Vm::new(0);
        drain_gc(&mut vm); // 排水预热
        let dead = vm.alloc_ordinary();
        vm.force_gc();
        assert!(vm.gc.is_free[dead.0 as usize]);
        let reused = vm.alloc_ordinary();
        assert_eq!(reused.0, dead.0, "新分配应复用空闲槽位");
        assert!(!vm.gc.is_free[reused.0 as usize]);
        assert_eq!(vm.gc_age(reused), Some(0), "复用槽位重置为年轻代");
    }

    /// 静态注册表持有的符号跨回收存活（root provider 验证）。
    #[test]
    fn registry_symbol_survives_gc() {
        let mut vm = Vm::new(0);
        let key = vm.alloc_string("gck".to_owned());
        let sym = vm
            .symbol_for(&[Value::Object(key)])
            .expect("symbol_for 不应失败");
        let first = sym.as_object().expect("应返回符号");
        vm.force_gc();
        assert!(
            !vm.gc.is_free[first.0 as usize],
            "for 注册表 provider 必须保活注册符号"
        );
        let key2 = vm.alloc_string("gck".to_owned());
        let again = vm
            .symbol_for(&[Value::Object(key2)])
            .expect("symbol_for 不应失败");
        assert_eq!(again, sym, "回收后注册表幂等仍成立");
    }

    /// minor 回收：年轻垃圾回收、老年代豁免、存活年龄增长。
    #[test]
    fn minor_collects_young_only_and_promotes() {
        let mut vm = Vm::new(0);
        drain_gc(&mut vm); // 排水预热
        // 老对象：经 major 晋升
        let old = vm.alloc_ordinary();
        vm.globals.insert("o".to_owned(), Value::Object(old));
        vm.force_gc();
        assert!(vm.gc_is_old(old));
        let young_dead = vm.alloc_ordinary();
        let young_keep = vm.alloc_ordinary();
        vm.globals
            .insert("yk".to_owned(), Value::Object(young_keep));
        vm.collect_minor_gc();
        assert!(vm.gc.is_free[young_dead.0 as usize], "年轻垃圾应回收");
        assert!(!vm.gc.is_free[young_keep.0 as usize], "年轻存活应保留");
        assert!(!vm.gc.is_free[old.0 as usize], "老年代豁免");
        assert!(
            vm.gc_age(young_keep).is_some_and(|a| a >= 1),
            "存活年龄应增长"
        );
    }

    /// 写屏障（卡表）：老年代对象写入年轻引用后，minor 不回收该年轻对象
    /// （脏卡次级根），且老对象所在卡重新置脏。
    #[test]
    fn write_barrier_protects_old_to_young() {
        let mut vm = Vm::new(0);
        drain_gc(&mut vm); // 排水预热
        let old = vm.alloc_ordinary();
        vm.globals.insert("o".to_owned(), Value::Object(old));
        vm.force_gc(); // old 晋升
        assert!(vm.gc_is_old(old));
        let young = vm.alloc_ordinary();
        let tag = vm.alloc_string("v".to_owned());
        let _ = vm.set_property(Value::Object(old), "t", Value::Object(tag));
        let _ = vm.set_property(Value::Object(old), "y", Value::Object(young));
        let card = (old.0 as usize) / CARD_SIZE as usize;
        assert!(
            vm.gc.cards.get(card).copied().unwrap_or(0) == 1,
            "写屏障应置脏卡"
        );
        vm.collect_minor_gc();
        assert!(
            !vm.gc.is_free[young.0 as usize],
            "脏卡次级根必须保住老→新引用"
        );
        let card = (old.0 as usize) / CARD_SIZE as usize;
        assert!(
            vm.gc.cards.get(card).copied().unwrap_or(0) == 1,
            "仍指向年轻应重新置脏"
        );
    }

    /// 自适应堆伸缩：存活率高 → 抬高 minor 阈值；存活率低 → 压回地板。
    #[test]
    fn adaptive_trigger_tracks_survival_rate() {
        let mut gc = GcState {
            minor_trigger: MINOR_TRIGGER_FLOOR,
            major_trigger: MINOR_TRIGGER_FLOOR * MAJOR_TRIGGER_RATIO,
            ..GcState::default()
        };
        // 全存活：阈值抬升（高于地板）
        gc.adapt_after_minor(10_000, 0);
        assert!(gc.minor_trigger > MINOR_TRIGGER_FLOOR, "全存活应抬高阈值");
        // 全死亡：阈值压回地板
        gc.adapt_after_minor(0, 10_000);
        assert_eq!(gc.minor_trigger, MINOR_TRIGGER_FLOOR, "全死亡应压回地板");
        // major 阈值保持倍率
        assert_eq!(gc.major_trigger, MINOR_TRIGGER_FLOOR * MAJOR_TRIGGER_RATIO);
    }

    /// 卡表粒度：不同卡的槽位互不影响，同卡共享一个字节。
    #[test]
    fn card_table_granularity() {
        let mut gc = GcState::default();
        gc.mark_card(0);
        gc.mark_card(CARD_SIZE * 3);
        assert_eq!(gc.cards[0], 1);
        assert_eq!(gc.cards[2], 0);
        assert_eq!(gc.cards[3], 1);
    }
}
