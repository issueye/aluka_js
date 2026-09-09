//! npm semver 版本与范围引擎（node-semver 标准模式语义的手工实现）。
//!
//! 不引 `semver` crate：其语法是 **Cargo 方言**（要求完整版本号、无 x-range、
//! 无连字符范围、无前导 `v` 容忍），与 npm 的 node-semver 语义存在系统性分歧。
//! 本模块按 node-semver 官方 README 语法实现，黄金用例与真实 `semver` 包
//! 对拍固化（见 `tests/semver_golden_test.rs`）。
//!
//! 支持面：`^` / `~` / 原语比较（`>= <= > < =`）/ x-range（`1.2.x` `*` `x`）/
//! 连字符范围（`1.2.3 - 2.3`）/ `||` 并集 / 前导 `v`=` 容忍 / 预发布
//! （NumericIdentifier < AlphanumericIdentifier、同 [M,m,p] 元组守卫规则）；
//! build 元数据参与解析、不参与比较。

use std::cmp::Ordering;
use std::fmt;

/// 预发布标识段：纯数字段按数值比较，否则按 ASCII 字典序。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreId {
    /// 纯数字段（按数值比较）
    Num(u64),
    /// 字母数字段（按 ASCII 字典序比较）
    Alnum(String),
}

impl PreId {
    fn parse(s: &str) -> Option<PreId> {
        if s.is_empty() {
            return None;
        }
        if s.bytes().all(|b| b.is_ascii_digit()) {
            // 前导零的数字段按规范非法（node-semver loose 之外拒绝）
            if s.len() > 1 && s.starts_with('0') {
                return None;
            }
            s.parse::<u64>().ok().map(PreId::Num)
        } else if s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            Some(PreId::Alnum(s.to_owned()))
        } else {
            None
        }
    }
}

impl fmt::Display for PreId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PreId::Num(n) => write!(f, "{n}"),
            PreId::Alnum(s) => write!(f, "{s}"),
        }
    }
}

/// 完整语义化版本：`major.minor.patch[-pre][+build]`。
#[derive(Debug, Clone, Eq)]
pub struct Version {
    /// 主版本号
    pub major: u64,
    /// 次版本号
    pub minor: u64,
    /// 修订号
    pub patch: u64,
    /// 预发布段（空 = 正式版）
    pub pre: Vec<PreId>,
    /// build 元数据（比较时忽略）
    pub build: Option<String>,
}

impl Version {
    /// 主次修订三元组（预发布守卫规则按元组同异判定）。
    fn tuple(&self) -> (u64, u64, u64) {
        (self.major, self.minor, self.patch)
    }

    /// 是否预发布版本。
    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }

    /// 解析完整版本号（容忍前导 `v` / `=`；预发布段与 build 元数据可选）。
    pub fn parse(s: &str) -> Result<Version, String> {
        let s = s.trim();
        let s = s.strip_prefix('=').unwrap_or(s);
        let s = s.strip_prefix('v').unwrap_or(s);
        let (core_rest, build) = match s.split_once('+') {
            Some((c, b)) => {
                if b.is_empty()
                    || !b
                        .bytes()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == b'.' || ch == b'-')
                {
                    return Err(format!("非法 build 元数据: {s}"));
                }
                (c, Some(b.to_owned()))
            }
            None => (s, None),
        };
        let (core, pre_str) = match core_rest.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (core_rest, None),
        };
        let nums: Vec<&str> = core.split('.').collect();
        if nums.len() != 3 {
            return Err(format!("版本号须为 major.minor.patch: {s}"));
        }
        let parse_num = |t: &str| -> Result<u64, String> {
            if t.is_empty()
                || !t.bytes().all(|b| b.is_ascii_digit())
                || (t.len() > 1 && t.starts_with('0'))
            {
                return Err(format!("非法版本号段: {s}"));
            }
            t.parse::<u64>().map_err(|_| format!("版本号溢出: {s}"))
        };
        let major = parse_num(nums[0])?;
        let minor = parse_num(nums[1])?;
        let patch = parse_num(nums[2])?;
        let pre = match pre_str {
            None => Vec::new(),
            Some(p) => {
                if p.is_empty() {
                    return Err(format!("空预发布段: {s}"));
                }
                p.split('.')
                    .map(PreId::parse)
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| format!("非法预发布段: {s}"))?
            }
        };
        Ok(Version {
            major,
            minor,
            patch,
            pre,
            build,
        })
    }

    /// 解析范围语境中的 partial：缺省段以 `x` 标记（`None`）。
    fn parse_partial(s: &str) -> Result<Partial, String> {
        let s = s.trim();
        let s = s.strip_prefix('=').unwrap_or(s);
        let s = s.strip_prefix('v').unwrap_or(s);
        let (core_rest, build) = match s.split_once('+') {
            Some((c, b)) => (c, Some(b.to_owned())),
            None => (s, None),
        };
        let (core, pre_str) = match core_rest.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (core_rest, None),
        };
        let segs: Vec<&str> = core.split('.').collect();
        if segs.len() > 3 {
            return Err(format!("版本段过多: {s}"));
        }
        let mut nums = [None; 3];
        for (i, seg) in segs.iter().enumerate() {
            if *seg == "x" || *seg == "X" || *seg == "*" {
                break;
            }
            if seg.is_empty() || !seg.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!("非法版本段: {s}"));
            }
            // 范围语境允许非严格形态（如 `01`）——按数值解释
            nums[i] = Some(seg.parse::<u64>().map_err(|_| format!("版本号溢出: {s}"))?);
        }
        let pre = match pre_str {
            None => Vec::new(),
            Some(p) => p
                .split('.')
                .map(PreId::parse)
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| format!("非法预发布段: {s}"))?,
        };
        // 缺省段之后不允许再出现显式段（`1.x.3` 非法）
        let present = nums.iter().take_while(|n| n.is_some()).count();
        if present < segs.len() {
            // 检查 x 之后是否还有数字段（上面 break 前的段均合法）
            for seg in &segs[present + 1..] {
                let is_x = *seg == "x" || *seg == "X" || *seg == "*";
                if !is_x {
                    return Err(format!("x 段之后不允许显式段: {s}"));
                }
            }
        }
        let has_pre = !pre.is_empty() || build.is_some();
        if has_pre && present < 3 {
            return Err(format!("不完整版本号不允许携带预发布/build 段: {s}"));
        }
        Ok(Partial { nums, pre, build })
    }
}

// 版本比较：major → minor → patch → 预发布段（无 pre > 有 pre）。
// build 元数据不参与。
impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.tuple().cmp(&other.tuple()).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    for (a, b) in self.pre.iter().zip(other.pre.iter()) {
                        let ord = match (a, b) {
                            (PreId::Num(x), PreId::Num(y)) => x.cmp(y),
                            (PreId::Num(_), PreId::Alnum(_)) => Ordering::Less,
                            (PreId::Alnum(_), PreId::Num(_)) => Ordering::Greater,
                            (PreId::Alnum(x), PreId::Alnum(y)) => x.cmp(y),
                        };
                        if ord != Ordering::Equal {
                            return ord;
                        }
                    }
                    self.pre.len().cmp(&other.pre.len())
                }
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            let joined: Vec<String> = self.pre.iter().map(ToString::to_string).collect();
            write!(f, "-{}", joined.join("."))?;
        }
        if let Some(b) = &self.build {
            write!(f, "+{b}")?;
        }
        Ok(())
    }
}

/// 范围语境中的 partial：三段均可缺省（x / 缺写）。
#[derive(Debug, Clone)]
struct Partial {
    /// 各段（None = x / 缺省）
    nums: [Option<u64>; 3],
    /// 预发布段（仅完整版本允许携带）
    pre: Vec<PreId>,
    /// build 元数据
    build: Option<String>,
}

impl Partial {
    fn get(&self, i: usize) -> Option<u64> {
        self.nums[i]
    }
}

/// 比较算子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompOp {
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `=`（精确）
    Eq,
}

/// 单个比较器：`op 全量版本`。
#[derive(Debug, Clone)]
pub struct Comparator {
    /// 比较算子
    pub op: CompOp,
    /// 比较目标版本（partial 已按语义规则补全）
    pub ver: Version,
}

impl Comparator {
    fn test(&self, v: &Version) -> bool {
        match self.op {
            CompOp::Lt => v < &self.ver,
            CompOp::Le => v <= &self.ver,
            CompOp::Gt => v > &self.ver,
            CompOp::Ge => v >= &self.ver,
            CompOp::Eq => v == &self.ver,
        }
    }
}

/// 范围：`||` 并集下的比较器合取集列表。
#[derive(Debug, Clone)]
pub struct Range {
    /// 各 OR 分支（每支为比较器合取）
    sets: Vec<Vec<Comparator>>,
}

impl Range {
    /// 解析 npm 范围表达式。
    pub fn parse(s: &str) -> Result<Range, String> {
        let s = s.trim();
        let sets = s
            .split("||")
            .map(parse_simple_set)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Range { sets })
    }

    /// 版本是否落在范围内（含预发布同元组守卫规则）。
    pub fn satisfies(&self, v: &Version) -> bool {
        for set in &self.sets {
            if set.iter().all(|c| c.test(v)) {
                if !v.is_prerelease() {
                    return true;
                }
                // 预发布版本：仅当本分支存在「预发布比较器且元组相同」时才可满足
                // （node-semver satisfies 的预发布排除规则）
                if set
                    .iter()
                    .any(|c| c.ver.is_prerelease() && c.ver.tuple() == v.tuple())
                {
                    return true;
                }
            }
        }
        false
    }

    /// 全量候选中落在范围内的最高版本（npm 安装解析语义）。
    pub fn max_satisfying<'a, I: IntoIterator<Item = &'a Version>>(
        &self,
        candidates: I,
    ) -> Option<Version> {
        candidates
            .into_iter()
            .filter(|v| self.satisfies(v))
            .max()
            .cloned()
    }
}

/// 解析单个 OR 分支：比较器合取（连字符范围在此展开）。
fn parse_simple_set(s: &str) -> Result<Vec<Comparator>, String> {
    let s = s.trim();
    if s.is_empty() {
        // 空串 / `*` = 任意版本
        return Ok(vec![Comparator {
            op: CompOp::Ge,
            ver: Version {
                major: 0,
                minor: 0,
                patch: 0,
                pre: Vec::new(),
                build: None,
            },
        }]);
    }
    // 连字符范围：`partial - partial`（要求两侧留白）
    if let Some((lo_str, hi_str)) = split_hyphen(s) {
        let lo = Version::parse_partial(&lo_str)?;
        let hi = Version::parse_partial(&hi_str)?;
        let mut out = Vec::new();
        // 下界：缺省段补 0
        out.push(Comparator {
            op: CompOp::Ge,
            ver: Partial::to_version_lo(&lo),
        });
        // 上界：完整版本 → `<=` 含端点；缺省段 → bump 末位 `<`（node-semver
        // 语义：`1.2.3 - 2.3.4` = `>=1.2.3 <=2.3.4`；`- 2.3` = `<2.4.0-0`）
        match (hi.get(0), hi.get(1), hi.get(2)) {
            (Some(m), Some(n), Some(v)) => out.push(Comparator {
                op: CompOp::Le,
                ver: version_with_build(m, n, v, hi.pre.clone(), hi.build.clone()),
            }),
            (Some(m), Some(n), None) => out.push(Comparator {
                op: CompOp::Lt,
                ver: version(m, n + 1, 0, vec![PreId::Num(0)]),
            }),
            (Some(m), None, None) => out.push(Comparator {
                op: CompOp::Lt,
                ver: version(m + 1, 0, 0, vec![PreId::Num(0)]),
            }),
            _ => return Err(format!("连字符上界不允许全 x: {s}")),
        }
        return Ok(out);
    }
    let mut out = Vec::new();
    // 空白容忍预合并：`>= 1.2.3` / `^ 1.2.3` 的「算子独立成 token」形态
    // 与版本号重新拼接（node-semver 对运算符后留白宽容）
    let tokens: Vec<&str> = s.split_whitespace().collect();
    let mut merged: Vec<String> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if is_op_token(tokens[i]) && i + 1 < tokens.len() {
            merged.push(format!("{}{}", tokens[i], tokens[i + 1]));
            i += 2;
        } else {
            merged.push(tokens[i].to_owned());
            i += 1;
        }
    }
    for tok in &merged {
        out.extend(parse_simple(tok)?);
    }
    if out.is_empty() {
        return Err(format!("空范围分支: {s}"));
    }
    Ok(out)
}

/// 是否为「仅算子」的 token（待与后续版本 token 合并）。
fn is_op_token(t: &str) -> bool {
    matches!(t, ">" | "<" | ">=" | "<=" | "=" | "~" | "^" | "~>")
}

/// 切分连字符范围（` a - b `；单个 `-` 属版本预发布段，不在此列）。
fn split_hyphen(s: &str) -> Option<(String, String)> {
    let tokens: Vec<&str> = s.split_whitespace().collect();
    if tokens.len() >= 3 {
        if let Some(pos) = tokens.iter().position(|t| *t == "-") {
            let lo = tokens[..pos].join(" ");
            let hi = tokens[pos + 1..].join(" ");
            return Some((lo.trim().to_owned(), hi.trim().to_owned()));
        }
    }
    None
}

/// 解析单个 simple（原语 / 波浪 / 插入 / 裸 partial / `*`）。
fn parse_simple(tok: &str) -> Result<Vec<Comparator>, String> {
    let (op, rest) = if let Some(r) = tok.strip_prefix(">=") {
        (Some(CompOp::Ge), r)
    } else if let Some(r) = tok.strip_prefix("<=") {
        (Some(CompOp::Le), r)
    } else if let Some(r) = tok.strip_prefix("~>") {
        // 别名：`~>` = `~`
        (None, r)
    } else if let Some(r) = tok.strip_prefix('~') {
        (None, r)
    } else if let Some(r) = tok.strip_prefix('^') {
        (None, r)
    } else if let Some(r) = tok.strip_prefix('<') {
        (Some(CompOp::Lt), r)
    } else if let Some(r) = tok.strip_prefix('>') {
        (Some(CompOp::Gt), r)
    } else {
        (None, tok)
    };
    let rest = rest.trim();
    let is_tilde = tok.starts_with('~');
    let is_caret = tok.starts_with('^');
    let partial = Version::parse_partial(rest)?;
    // 全 x-range（`*` / `x` / `X` / 空）：任意版本（原语前缀下同）
    if partial.get(0).is_none() {
        if matches!(op, Some(CompOp::Eq)) || op.is_none() {
            return Ok(vec![Comparator {
                op: CompOp::Ge,
                ver: version(0, 0, 0, Vec::new()),
            }]);
        }
        return Err(format!("x-range 不允许携带比较算子: {tok}"));
    }
    // `^` 语义
    if is_caret {
        return Ok(caret_comparators(&partial));
    }
    // `~` 语义
    if is_tilde {
        return Ok(tilde_comparators(&partial));
    }
    let Some(op) = op else {
        // 裸 partial：x 段降级为区间，完整版为精确
        return Ok(partial_to_comparators(&partial));
    };
    match op {
        CompOp::Eq => Ok(partial_to_comparators(&partial)),
        CompOp::Ge => Ok(vec![Comparator {
            op: CompOp::Ge,
            ver: Partial::to_version_lo(&partial),
        }]),
        CompOp::Lt => Ok(vec![Comparator {
            op: CompOp::Lt,
            ver: Partial::to_version_lo(&partial),
        }]),
        CompOp::Gt | CompOp::Le => {
            // `>1.2` → `>=1.3.0`；`<=1.2` → `<1.3.0`；`>1` → `>=2.0.0`；`<=1` → `<2.0.0`
            let m = partial.get(0).expect("上方已排除全 x");
            if partial.get(1).is_none() {
                let bumped = version(m + 1, 0, 0, Vec::new());
                Ok(vec![Comparator {
                    op: if op == CompOp::Gt {
                        CompOp::Ge
                    } else {
                        CompOp::Lt
                    },
                    ver: bumped,
                }])
            } else if partial.get(2).is_none() && partial.pre.is_empty() {
                let minor = partial.get(1).expect("已判存在");
                let bumped = version(m, minor + 1, 0, Vec::new());
                Ok(vec![Comparator {
                    op: if op == CompOp::Gt {
                        CompOp::Ge
                    } else {
                        CompOp::Lt
                    },
                    ver: bumped,
                }])
            } else {
                // 完整版本（或带预发布）：保留算子原样
                let v = Partial::to_version_lo(&partial);
                Ok(vec![Comparator { op, ver: v }])
            }
        }
    }
}

/// 裸 partial / `=` partial → 比较器（x 段展开为区间；完整版精确）。
fn partial_to_comparators(p: &Partial) -> Vec<Comparator> {
    match (p.get(0), p.get(1), p.get(2)) {
        (Some(m), Some(n), Some(v)) => {
            let pre = p.pre.clone();
            vec![Comparator {
                op: CompOp::Eq,
                ver: version_with_build(m, n, v, pre, p.build.clone()),
            }]
        }
        (Some(m), Some(n), None) => {
            vec![
                Comparator {
                    op: CompOp::Ge,
                    ver: version(m, n, 0, Vec::new()),
                },
                Comparator {
                    op: CompOp::Lt,
                    ver: version(m, n + 1, 0, Vec::new()),
                },
            ]
        }
        (Some(m), None, None) => {
            vec![
                Comparator {
                    op: CompOp::Ge,
                    ver: version(m, 0, 0, Vec::new()),
                },
                Comparator {
                    op: CompOp::Lt,
                    ver: version(m + 1, 0, 0, Vec::new()),
                },
            ]
        }
        // 全 x 已在上层处理
        _ => unreachable!("全 x-range 由调用方处理"),
    }
}

/// `^` 语义：最左非零段锁定（`^1.2.3`→`>=1.2.3 <2.0.0`；`^0.2.3`→`<0.3.0`；
/// `^0.0.3`→`<0.0.4`；`^1.2`→`>=1.2.0 <1.3.0`；`^1`→`>=1.0.0 <2.0.0`；
/// `^0.0`→`>=0.0.0 <0.1.0`；`^0`→`>=0.0.0 <1.0.0`）。
fn caret_comparators(p: &Partial) -> Vec<Comparator> {
    let m = p.get(0).expect("调用方已排除全 x");
    let lower = Partial::to_version_lo(p);
    let upper = match (p.get(1), p.get(2)) {
        // 主版本非 0（或次段缺省）：升主版本
        (None, _) => version(m + 1, 0, 0, Vec::new()),
        (Some(n), None) => {
            if m == 0 {
                // ^0.0 / ^0.x：升次版本
                version(0, n + 1, 0, Vec::new())
            } else {
                version(m + 1, 0, 0, Vec::new())
            }
        }
        (Some(n), Some(v)) => {
            if m > 0 {
                version(m + 1, 0, 0, Vec::new())
            } else if n > 0 {
                version(0, n + 1, 0, Vec::new())
            } else {
                // ^0.0.v：升修订号（预发布保留）
                version(0, 0, v + 1, Vec::new())
            }
        }
    };
    vec![
        Comparator {
            op: CompOp::Ge,
            ver: lower,
        },
        Comparator {
            op: CompOp::Lt,
            ver: upper,
        },
    ]
}

/// `~` 语义：锁定主版本与次版本（`~1.2.3`→`>=1.2.3 <1.3.0`；
/// `~1.2`→同上补零；`~1`→`>=1.0.0 <2.0.0`）。
fn tilde_comparators(p: &Partial) -> Vec<Comparator> {
    let m = p.get(0).expect("调用方已排除全 x");
    let lower = Partial::to_version_lo(p);
    let upper = match p.get(1) {
        Some(n) => version(m, n + 1, 0, Vec::new()),
        None => version(m + 1, 0, 0, Vec::new()),
    };
    vec![
        Comparator {
            op: CompOp::Ge,
            ver: lower,
        },
        Comparator {
            op: CompOp::Lt,
            ver: upper,
        },
    ]
}

impl Partial {
    /// 下界补全：缺省段补 0，保留预发布。
    fn to_version_lo(&self) -> Version {
        version_with_build(
            self.get(0).unwrap_or(0),
            self.get(1).unwrap_or(0),
            self.get(2).unwrap_or(0),
            self.pre.clone(),
            self.build.clone(),
        )
    }
}

/// 快捷构造版本。
fn version(major: u64, minor: u64, patch: u64, pre: Vec<PreId>) -> Version {
    Version {
        major,
        minor,
        patch,
        pre,
        build: None,
    }
}

/// 快捷构造版本（带 build 元数据）。
fn version_with_build(
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<PreId>,
    build: Option<String>,
) -> Version {
    Version {
        major,
        minor,
        patch,
        pre,
        build,
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 调试/日志用的人类可读形态（非 npm 原文还原）
        for (i, set) in self.sets.iter().enumerate() {
            if i > 0 {
                write!(f, " || ")?;
            }
            let parts: Vec<String> = set
                .iter()
                .map(|c| match c.op {
                    CompOp::Lt => format!("<{}", c.ver),
                    CompOp::Le => format!("<={}", c.ver),
                    CompOp::Gt => format!(">{}", c.ver),
                    CompOp::Ge => format!(">={}", c.ver),
                    CompOp::Eq => format!("{}", c.ver),
                })
                .collect();
            write!(f, "{}", parts.join(" "))?;
        }
        Ok(())
    }
}
