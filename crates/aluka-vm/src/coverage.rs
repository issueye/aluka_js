//! LCOV 行覆盖计数（M5.4）。
//!
//! 数据来源：编译器在 `line_coverage` 开启时把每条语句的起始
//! `(指令索引, 源码行号)` 登记进函数模板的 `line_table`（不参与 `.bc`
//! 序列化——行覆盖是 `aluka test --test-reporter=lcov` 的进程内编译特性）。
//!
//! 运行期：`Vm.coverage` 为 `Some` 时，解释器主循环每条指令做一次
//! `Option` 判定（关闭态零成本），命中时二分定位「pc ≤ 当前 pc 的最后一条
//! 行表项」并把该行计数 +1（跳转/循环天然被二分覆盖，无单调性假设）。
//!
//! 报告形态：LCOV tracefile 文本（TN/SF/FN/FNDA/FNF/FNH/DA/LF/LH/
//! end_of_record）。BRDA 分支覆盖不支持（引擎无分支级插桩，登记偏离）。

use aluka_bytecode::{BytecodeModule, FuncTemplate};

/// 覆盖计数状态（挂在 `Vm.coverage`）。
pub struct Coverage {
    /// 每函数行表（与 `module.functions` 平行；空表 = 该函数无覆盖插桩）
    pub tables: Vec<Vec<(u32, u32)>>,
    /// 每函数行命中计数（与 `tables` 条目平行，惰性分配）
    pub counts: Vec<Vec<u32>>,
    /// 当前帧已解析的表缓存：(函数索引, 表长度)
    current: Option<(usize, usize)>,
    /// 迁移计数用的「上一命中条目」：(函数索引, 条目下标)——只有进入
    /// **不同**条目（语句切换）才计数，把指令命中数归约为语句执行次数。
    /// 帧切换时保存/恢复（嵌套调用返回回同一语句不重计）
    pub last_hit: Option<(usize, usize)>,
    /// 当前帧函数索引（0 = main 顶层；`invoke_function` 帧切换时更新；
    /// 生成器恢复点同样维护——行归属关键状态）
    pub cur_func: i64,
}

impl Coverage {
    /// 从编译产物构建覆盖状态（函数模板自带行表；`.bc` 加载产物表为空）。
    #[must_use]
    pub fn from_module(module: &BytecodeModule) -> Self {
        Self {
            tables: module
                .functions
                .iter()
                .map(|f| f.line_table.clone())
                .collect(),
            counts: vec![Vec::new(); module.functions.len()],
            current: None,
            last_hit: None,
            cur_func: 0,
        }
    }

    /// 解释器每指令调用（主循环在 `coverage` 为 `Some` 时才进入）。
    pub fn on_instruction(&mut self, pc: usize) {
        let fi_u = if self.cur_func < 0 {
            usize::MAX
        } else {
            self.cur_func as usize
        };
        let func_idx = self.cur_func;
        let _ = func_idx;
        let fi = fi_u;
        if fi >= self.tables.len() || self.tables[fi].is_empty() {
            self.current = None;
            return;
        }
        // 帧切换 / 表变更后重新解析当前表
        if !matches!(self.current, Some((f, len)) if f == fi && len == self.tables[fi].len()) {
            self.current = Some((fi, self.tables[fi].len()));
        }
        let table = &self.tables[fi];
        // 计数惰性分配
        if self.counts[fi].len() != table.len() {
            self.counts[fi] = vec![0; table.len()];
        }
        // 二分：最后一个 `pc <= 当前 pc` 的行表项（语句起始 ≤ 当前指令）
        let pc32 = pc as u32;
        let mut lo = 0usize;
        let mut hi = table.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            if table[mid].0 <= pc32 {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == 0 {
            return;
        }
        let entry = lo - 1;
        // 迁移计数：进入不同条目（语句切换）才 +1——指令级命中会随语句
        // 长度膨胀，而 LCOV DA 语义是「该行语句的执行次数」
        if self.last_hit != Some((fi, entry)) {
            self.last_hit = Some((fi, entry));
            self.counts[fi][entry] += 1;
        }
    }

    /// 生成 LCOV tracefile 文本（按 `SF` 文件聚合，行计数跨函数合并）。
    ///
    /// `test_name` 写入 `TN:`；无任何插桩数据时返回空串。
    #[must_use]
    pub fn generate_lcov(
        &self,
        module: &BytecodeModule,
        test_name: &str,
        sf_override: Option<&str>,
    ) -> String {
        let templates: Vec<aluka_bytecode::FuncTemplate> = module.functions.to_vec();
        self.generate_lcov_from(&templates, test_name, sf_override)
    }

    /// compose 管道路径：直接基于运行中的函数模板生成（`SF` 取模板
    /// `source_file`，进程内编译为空时回退 `compiled.js`——登记偏离）。
    #[must_use]
    pub fn generate_lcov_from_current(
        &self,
        templates: &[std::rc::Rc<FuncTemplate>],
        test_name: &str,
    ) -> String {
        let borrowed: Vec<FuncTemplate> = templates.iter().map(|t| (**t).clone()).collect();
        self.generate_lcov_from(&borrowed, test_name, None)
    }

    /// LCOV 聚合核心：按 `SF` 文件聚合，行计数跨函数合并。
    fn generate_lcov_from(
        &self,
        functions: &[FuncTemplate],
        test_name: &str,
        sf_override: Option<&str>,
    ) -> String {
        // (文件, 行) → 命中数；函数清单：(文件, 首行, 名, 执行数)
        let mut files: Vec<String> = Vec::new();
        let mut per_file: std::collections::BTreeMap<
            String,
            std::collections::BTreeMap<u32, (u64, u64)>,
        > = std::collections::BTreeMap::new(); // line -> (count, 已计行标记占位)
        let mut fn_list: Vec<(String, u32, String, u64)> = Vec::new();

        for (fi, func) in functions.iter().enumerate() {
            let table = &self.tables[fi];
            if table.is_empty() {
                continue;
            }
            let counts = &self.counts[fi];
            // 函数清单：首语句行 + 执行数 = 首条目计数
            let first_line = table[0].1;
            let first_count = counts.first().copied().unwrap_or(0) as u64;
            fn_list.push((
                func.source_file.clone(),
                first_line,
                func.name.clone(),
                first_count,
            ));
            let file_map = per_file.entry(func.source_file.clone()).or_default();
            if !files.contains(&func.source_file) {
                files.push(func.source_file.clone());
            }
            for ((pc32, line), c) in table.iter().zip(counts.iter()) {
                let _ = pc32;
                let entry = file_map.entry(*line).or_insert((0, 0));
                entry.0 += u64::from(*c);
                entry.1 = 1;
            }
        }
        if per_file.is_empty() {
            return String::new();
        }

        let mut out = String::new();
        out.push_str(&format!("TN:{test_name}\n"));
        // BTreeMap 按 SF 排序，保证输出确定性
        for (file, lines) in &per_file {
            out.push_str(&format!("SF:{}\n", sf_override.unwrap_or(file)));
            // FN/FNDA（本文件的函数）
            let mut fn_found = 0u32;
            let mut fn_hit = 0u32;
            for (f, first_line, name, count) in &fn_list {
                if *f == *file {
                    fn_found += 1;
                    if *count > 0 {
                        fn_hit += 1;
                    }
                    out.push_str(&format!("FN:{first_line},{name}\n"));
                    out.push_str(&format!("FNDA:{count},{name}\n"));
                }
            }
            out.push_str(&format!("FNF:{fn_found}\n"));
            out.push_str(&format!("FNH:{fn_hit}\n"));
            let mut lines_found = 0u64;
            let mut lines_hit = 0u64;
            for (line, (count, _)) in lines {
                lines_found += 1;
                if *count > 0 {
                    lines_hit += 1;
                }
                out.push_str(&format!("DA:{line},{count}\n"));
            }
            out.push_str(&format!("LF:{lines_found}\n"));
            out.push_str(&format!("LH:{lines_hit}\n"));
            out.push_str("end_of_record\n");
        }
        out
    }
}
