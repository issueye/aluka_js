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
//! # 当前形态（对 ADR 的收敛偏差，已记录）
//!
//! minor（年轻代，4k 分配阈值）+ major（全堆，20k 阈值）双触发，写屏障minor 回收与
//! 覆盖全部「老写新」变异点，记忆集作为 minor 次级根。
//! 全部「老对象写入年轻引用」的变异点（set_property/数组元素/Map/事件表/
//! upvalue 写入…），变异点审计完成前启用有悬垂风险——见总 TODO M3 条目。
//! age 计数与晋升逻辑已实现，major 存活对象直接晋升。

use crate::heap::HeapObject;
use crate::interpreter::Vm;
use crate::value::Value;
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

/// 触发 major 回收的分配次数阈值。
const MAJOR_TRIGGER: u32 = 20_000;

/// 触发 minor 回收的分配次数阈值（年轻代高频回收）。
const MINOR_TRIGGER: u32 = 4_000;

/// GC 侧表与统计。
#[derive(Debug, Default)]
pub(crate) struct GcState {
    /// 与 `Vm.heap` 平行的对象年龄（分配序号对齐；Free 槽位无意义）
    pub(crate) ages: Vec<u8>,
    /// 槽位是否为清扫后的空闲占位
    pub(crate) is_free: Vec<bool>,
    /// 年轻代空闲槽位（清扫产生，分配复用）
    pub(crate) young_free: Vec<u32>,
    /// 老年代空闲槽位
    pub(crate) old_free: Vec<u32>,
    /// 记忆集：写入过年轻引用的老年代对象（写屏障登记，minor 次级根）
    pub(crate) remembered: Vec<u32>,
    /// 自上次 minor 回收以来的分配次数（minor 触发用）
    pub(crate) allocs_since_minor: u32,
    /// 自上次 major 回收以来的分配次数（major 触发用）
    pub(crate) allocs_since_major: u32,
    /// 累计分配对象数
    pub(crate) allocated: u64,
    /// 累计回收对象数
    pub(crate) reclaimed: u64,
    /// major 回收次数
    pub(crate) major_collections: u64,
    /// minor 回收次数
    pub(crate) minor_collections: u64,
}

impl GcState {
    /// 分配记账；返回 (达到 minor 阈值, 达到 major 阈值)。
    pub(crate) fn on_alloc(&mut self) -> (bool, bool) {
        self.allocated += 1;
        self.allocs_since_minor += 1;
        self.allocs_since_major += 1;
        (
            self.allocs_since_minor >= MINOR_TRIGGER,
            self.allocs_since_major >= MAJOR_TRIGGER,
        )
    }
}

impl Vm {
    /// 对象当前年龄。
    /// minor 机制当前仅测试路径使用（生产 major-only，见模块文档偏差记录）。
    #[allow(dead_code)]
    pub(crate) fn gc_age(&self, r: ObjectRef) -> Option<u8> {
        let idx = r.0 as usize;
        if idx < self.gc.ages.len() && !self.gc.is_free[idx] {
            self.gc.ages.get(idx).copied()
        } else {
            None
        }
    }

    /// 是否为老年代对象（年龄达到晋升阈值）。
    /// minor 机制当前仅测试路径使用（生产 major-only，见模块文档偏差记录）。
    #[allow(dead_code)]
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
            self.error_ctor,
            self.array_ctor,
            self.object_ctor,
            self.promise_ctor,
            self.map_ctor,
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
            }
        }
        // 挂起 async 帧恢复登记
        for r in self.promise_resumes.values() {
            self.push_resume_roots(out, r);
        }
        // 模块导出缓存与内置注册表单例
        for v in self.module_exports.values() {
            out.push(*v);
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
    }

    /// 执行一次 major 全堆标记-清除。返回回收对象数。
    pub(crate) fn collect_major_gc(&mut self) -> u64 {
        let roots = self.build_gc_roots();
        let mut marked = vec![false; self.heap.len()];
        for root in roots.iter() {
            if let Value::Object(r) = root {
                self.mark_all(r.0, &mut marked);
            }
        }
        let mut reclaimed = 0u64;
        for (idx, slot) in self.heap.iter_mut().enumerate() {
            if marked[idx] || self.gc.is_free[idx] {
                continue;
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
        reclaimed
    }

    /// 执行一次 minor 回收（只清年轻代，写屏障记忆集保护老→新引用）。
    ///
    /// **默认不启用**（`minor_enabled`）：变异点写屏障审计完成前启用有悬垂
    /// 风险。测试经 [`Vm::gc_set_minor_enabled`] 显式开启验证机制本身。
    /// minor 机制当前仅测试路径使用（生产 major-only，见模块文档偏差记录）。
    #[allow(dead_code)]
    pub(crate) fn collect_minor_gc(&mut self) -> u64 {
        let roots = self.build_gc_roots();
        let mut marked = vec![false; self.heap.len()];
        for root in roots.iter() {
            if let Value::Object(r) = root {
                self.mark_young(r.0, &mut marked);
            }
        }
        // 记忆集：老年代对象的年轻引用是次级根；仍指向年轻的重新登记
        let remembered = std::mem::take(&mut self.gc.remembered);
        for old_idx in remembered {
            if self
                .gc
                .is_free
                .get(old_idx as usize)
                .copied()
                .unwrap_or(true)
            {
                continue;
            }
            let mut young_targets: Vec<u32> = Vec::new();
            if let Some(obj) = self.heap.get(old_idx as usize) {
                obj.trace_refs(|t| {
                    if !self.gc.is_free.get(t as usize).copied().unwrap_or(true)
                        && self.gc.ages.get(t as usize).copied().unwrap_or(PROMOTE_AGE)
                            < PROMOTE_AGE
                    {
                        young_targets.push(t);
                    }
                });
            }
            for t in young_targets {
                self.mark_young(t, &mut marked);
            }
            if let Some(obj) = self.heap.get(old_idx as usize) {
                let mut still = false;
                obj.trace_refs(|t| {
                    if !self.gc.is_free.get(t as usize).copied().unwrap_or(true)
                        && self.gc.ages.get(t as usize).copied().unwrap_or(PROMOTE_AGE)
                            < PROMOTE_AGE
                    {
                        still = true;
                    }
                });
                if still {
                    self.gc.remembered.push(old_idx);
                }
            }
        }
        let mut reclaimed = 0u64;
        for (idx, slot) in self.heap.iter_mut().enumerate() {
            if marked[idx] || self.gc.is_free[idx] {
                continue;
            }
            if self.gc.ages.get(idx).copied().unwrap_or(PROMOTE_AGE) >= PROMOTE_AGE {
                continue; // 老年代不参与 minor
            }
            self.gc.young_free.push(idx as u32);
            *slot = HeapObject::Free;
            self.gc.is_free[idx] = true;
            reclaimed += 1;
        }
        // 存活年轻对象年龄 +1（达到阈值自然晋升）
        for (idx, age) in self.gc.ages.iter_mut().enumerate() {
            if !self.gc.is_free[idx]
                && *age < PROMOTE_AGE
                && marked.get(idx).copied().unwrap_or(false)
            {
                *age += 1;
            }
        }
        self.gc.minor_collections += 1;
        self.gc.reclaimed += reclaimed;
        self.gc.allocs_since_minor = 0;
        reclaimed
    }

    /// minor 回收开关（测试用；生产路径保持 major-only）。
    /// minor 机制当前仅测试路径使用（生产 major-only，见模块文档偏差记录）。
    #[allow(dead_code)]
    /// 写屏障：老年代容器写入年轻代引用时记入记忆集（minor 回收的次级根）。
    /// **全部「老写新」变异点必须调用**（set_property / Map.set / 事件监听 /
    /// Readable 缓冲 / Promise 处理器 adoption），漏调用 = minor 悬垂。
    pub(crate) fn gc_write_barrier(&mut self, container: ObjectRef, val: Value) {
        if !self.gc_is_old(container) {
            return;
        }
        let young_target = match val {
            Value::Object(r) => {
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
        if young_target && !self.gc.remembered.contains(&container.0) {
            self.gc.remembered.push(container.0);
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
    /// minor 机制当前仅测试路径使用（生产 major-only，见模块文档偏差记录）。
    #[allow(dead_code)]
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::HeapObject;

    #[test]
    fn promote_age_constant_matches_adr() {
        assert_eq!(PROMOTE_AGE, 2, "ADR 原型 A：存活 2 次 minor 晋升");
    }

    /// 无引用的分配在 major 后被回收（槽位转 Free 并入 free-list）。
    #[test]
    fn unreferenced_object_is_reclaimed() {
        let mut vm = Vm::new(0);
        vm.force_gc(); // 预热：清掉引导期残留，隔离被测分配
        let dead = vm.alloc_ordinary();
        assert!(!vm.gc.is_free[dead.0 as usize]);
        let reclaimed = vm.force_gc();
        assert_eq!(reclaimed, 1);
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
        vm.force_gc(); // 预热
        let root = vm.alloc_ordinary();
        let mid = vm.alloc_ordinary();
        let dead = vm.alloc_ordinary();
        let _ = vm.set_property(Value::Object(root), "mid", Value::Object(mid));
        let tag = vm.alloc_string("v".to_owned());
        let _ = vm.set_property(Value::Object(mid), "tag", Value::Object(tag));
        vm.globals.insert("r".to_owned(), Value::Object(root));
        vm.force_gc();
        assert!(!vm.gc.is_free[root.0 as usize]);
        assert!(!vm.gc.is_free[mid.0 as usize]);
        assert!(vm.gc.is_free[dead.0 as usize], "链外对象应被回收");
    }

    /// 循环结构无外部根时被回收（追踪式 GC 的核心价值）。
    #[test]
    fn cyclic_garbage_is_collected() {
        let mut vm = Vm::new(0);
        vm.force_gc(); // 预热
        let a = vm.alloc_ordinary();
        let b = vm.alloc_ordinary();
        let _ = vm.set_property(Value::Object(a), "b", Value::Object(b));
        let _ = vm.set_property(Value::Object(b), "a", Value::Object(a));
        let before = vm.gc.reclaimed;
        vm.force_gc();
        assert_eq!(vm.gc.reclaimed - before, 2, "环上两个垃圾都应回收");
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
        vm.force_gc(); // 预热
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
        let first = match sym {
            Value::Object(r) => r,
            _ => panic!("应返回符号"),
        };
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
        vm.force_gc(); // 预热
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

    /// 写屏障：老年代对象写入年轻引用后，minor 不回收该年轻对象（记忆集
    /// 次级根），且老对象重新登记。
    #[test]
    fn write_barrier_protects_old_to_young() {
        let mut vm = Vm::new(0);
        vm.force_gc(); // 预热
        let old = vm.alloc_ordinary();
        vm.globals.insert("o".to_owned(), Value::Object(old));
        vm.force_gc(); // old 晋升
        assert!(vm.gc_is_old(old));
        let young = vm.alloc_ordinary();
        let tag = vm.alloc_string("v".to_owned());
        let _ = vm.set_property(Value::Object(old), "t", Value::Object(tag));
        let _ = vm.set_property(Value::Object(old), "y", Value::Object(young));
        assert!(vm.gc.remembered.contains(&old.0), "写屏障应登记老容器");
        vm.collect_minor_gc();
        assert!(!vm.gc.is_free[young.0 as usize], "记忆集必须保住老→新引用");
        assert!(vm.gc.remembered.contains(&old.0), "仍指向年轻应重新登记");
    }
}
