//! 回溯式匹配引擎：CPS 续延 + 步数预算护栏。
//!
//! 输入文本以 `char` 序列参与匹配，返回的区间索引以 **char 计**；
//! UTF-16 code unit 换算由调用方借助 [`crate::utf16_offset`] 完成。

use crate::RegexError;
use crate::parser::{self, ClassItem, Node};

/// CPS 续延：匹配推进到 `pos`、捕获状态 `caps`，失败返回 `false` 触发回溯。
type Cont<'c> = &'c dyn Fn(usize, &mut Caps) -> bool;

/// 捕获组状态（索引 0 为全匹配区间，1..=n 为捕获组）。
type Caps = Vec<Option<(usize, usize)>>;

/// 回溯预算（匹配步数上限），防止病态模式把死循环伪装成长时间计算。
const BACKTRACK_BUDGET: u32 = 1_000_000;

/// 编译完成的正则表达式。
pub struct Regex {
    root: Node,
    group_count: usize,
    /// 捕获组命名（下标 gi-1 对应组号 gi；未命名的组为 `None`）。
    group_names: Vec<Option<String>>,
    ignore_case: bool,
    multiline: bool,
}

/// 一次成功匹配：全匹配区间与捕获组（索引以 char 计）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchResult {
    /// 全匹配起始（含）
    pub start: usize,
    /// 全匹配结束（不含）
    pub end: usize,
    /// 捕获组 1..=n 的区间（`None` 表示该组未参与匹配）
    pub groups: Vec<Option<(usize, usize)>>,
}

impl Regex {
    /// 编译正则：`flags` 中 `i` 忽略大小写、`m` 多行锚点（`^`/`$` 匹配
    /// 行边界）；`g`/`y` 记录但不改变本引擎语义。
    ///
    /// # Errors
    /// 模式语法错误时返回 [`RegexError::Syntax`]。
    pub fn compile(pattern: &str, flags: &str) -> Result<Self, RegexError> {
        let ignore_case = flags.contains('i');
        let multiline = flags.contains('m');
        let parsed = parser::parse(pattern)?;
        Ok(Self {
            root: parsed.root,
            group_count: parsed.group_count,
            group_names: parsed.group_names,
            ignore_case,
            multiline,
        })
    }

    /// 捕获组命名表（下标 gi-1 对应组号 gi）。
    #[must_use]
    pub fn group_names(&self) -> &[Option<String>] {
        &self.group_names
    }

    /// 捕获组个数。
    #[must_use]
    pub fn group_count(&self) -> usize {
        self.group_count
    }

    /// 在输入中查找第一个匹配（JS 非全局 `exec`/`test` 语义）。
    ///
    /// # Errors
    /// 回溯预算耗尽时返回 [`RegexError::BacktrackLimit`]——绝不把超限折叠成
    /// 「无匹配」。
    pub fn find(&self, input: &str) -> Result<Option<MatchResult>, RegexError> {
        self.find_at(input, 0)
    }

    /// 从 `start`（char 索引）起查找第一个匹配（`lastIndex` 驱动的
    /// 全局 `exec` 迭代用）。
    ///
    /// # Errors
    /// 回溯预算耗尽时返回 [`RegexError::BacktrackLimit`]。
    pub fn find_at(&self, input: &str, start: usize) -> Result<Option<MatchResult>, RegexError> {
        let chars: Vec<char> = input.chars().collect();
        let ctx = MatchCtx {
            input: &chars,
            steps: std::cell::Cell::new(BACKTRACK_BUDGET),
            exceeded: std::cell::Cell::new(false),
        };
        let last_end: std::cell::Cell<Option<usize>> = std::cell::Cell::new(None);
        for pos in start..=chars.len() {
            let mut caps = vec![None; self.group_count + 1];
            let matched = self.match_seq(
                std::slice::from_ref(&self.root),
                pos,
                &ctx,
                &mut caps,
                &|end, _| {
                    last_end.set(Some(end));
                    true
                },
            );
            if ctx.exceeded.get() {
                return Err(RegexError::BacktrackLimit);
            }
            if matched {
                let end = last_end.get().expect("matched 要求 cont 已记录终点");
                return Ok(Some(MatchResult {
                    start: pos,
                    end,
                    groups: caps[1..].to_vec(),
                }));
            }
        }
        Ok(None)
    }

    /// 是否存在匹配（`test` 语义）。
    ///
    /// # Errors
    /// 回溯预算耗尽时返回 [`RegexError::BacktrackLimit`]。
    pub fn test(&self, input: &str) -> Result<bool, RegexError> {
        Ok(self.find(input)?.is_some())
    }

    /// 按序匹配节点序列，全部成功后调用 `cont`（CPS 续延，失败回溯）。
    fn match_seq(
        &self,
        nodes: &[Node],
        pos: usize,
        ctx: &MatchCtx<'_>,
        caps: &mut Caps,
        cont: Cont<'_>,
    ) -> bool {
        ctx.step();
        if ctx.exceeded.get() {
            return false;
        }
        match nodes.split_first() {
            None => cont(pos, caps),
            Some((first, rest)) => self.match_node(first, pos, ctx, caps, &|p, c| {
                self.match_seq(rest, p, ctx, c, cont)
            }),
        }
    }

    /// 匹配单个节点。
    fn match_node(
        &self,
        node: &Node,
        pos: usize,
        ctx: &MatchCtx<'_>,
        caps: &mut Caps,
        cont: Cont<'_>,
    ) -> bool {
        ctx.step();
        if ctx.exceeded.get() {
            return false;
        }
        match node {
            Node::Char(c) => {
                if self.char_eq(ctx.input.get(pos).copied(), *c) {
                    cont(pos + 1, caps)
                } else {
                    false
                }
            }
            Node::Any => {
                if pos < ctx.input.len() {
                    cont(pos + 1, caps)
                } else {
                    false
                }
            }
            Node::Class { negated, items } => {
                let Some(&ch) = ctx.input.get(pos) else {
                    return false;
                };
                let hit = items.iter().any(|item| self.class_item_matches(item, ch));
                if hit != *negated {
                    cont(pos + 1, caps)
                } else {
                    false
                }
            }
            Node::Start => {
                // `m` 标志：`^` 另行匹配「前一字符为 \n」的行首
                let at_line_start = self.multiline && pos > 0 && ctx.input[pos - 1] == '\n';
                if pos == 0 || at_line_start {
                    cont(pos, caps)
                } else {
                    false
                }
            }
            Node::End => {
                // `m` 标志：`$` 另行匹配「下一字符为 \n」的行尾
                let at_line_end = self.multiline && ctx.input.get(pos) == Some(&'\n');
                if pos == ctx.input.len() || at_line_end {
                    cont(pos, caps)
                } else {
                    false
                }
            }
            Node::Group { index, node } => match index {
                None => self.match_node(node, pos, ctx, caps, cont),
                Some(gi) => {
                    let gi = *gi;
                    let open = pos;
                    let saved = caps[gi];
                    let ok = self.match_node(node, pos, ctx, caps, &|p, c| {
                        let prev = c[gi];
                        c[gi] = Some((open, p));
                        if cont(p, c) {
                            true
                        } else {
                            c[gi] = prev;
                            false
                        }
                    });
                    if !ok {
                        caps[gi] = saved;
                    }
                    ok
                }
            },
            Node::Concat(items) => self.match_seq(items, pos, ctx, caps, cont),
            Node::Alt(branches) => {
                for branch in branches {
                    if self.match_node(branch, pos, ctx, caps, cont) {
                        return true;
                    }
                }
                false
            }
            Node::Repeat {
                node,
                min,
                max,
                greedy,
            } => self.match_repeat(node, *min, *max, *greedy, pos, ctx, caps, 0, cont),
        }
    }

    /// 量词重复匹配（`count` 为已重复次数）。
    #[allow(clippy::too_many_arguments)]
    fn match_repeat(
        &self,
        node: &Node,
        min: u32,
        max: Option<u32>,
        greedy: bool,
        pos: usize,
        ctx: &MatchCtx<'_>,
        caps: &mut Caps,
        count: u32,
        cont: Cont<'_>,
    ) -> bool {
        ctx.step();
        if ctx.exceeded.get() {
            return false;
        }
        let can_more = max.is_none_or(|m| count < m);
        if count < min {
            return self.match_node(node, pos, ctx, caps, &|p, c| {
                self.match_repeat(node, min, max, greedy, p, ctx, c, count + 1, cont)
            });
        }
        if greedy {
            if can_more
                && self.match_node(node, pos, ctx, caps, &|p, c| {
                    // 空匹配不再扩展，防止 `(a?)*` 类模式无限回溯
                    if p == pos {
                        return false;
                    }
                    self.match_repeat(node, min, max, greedy, p, ctx, c, count + 1, cont)
                })
            {
                return true;
            }
            cont(pos, caps)
        } else {
            if cont(pos, caps) {
                return true;
            }
            if can_more {
                return self.match_node(node, pos, ctx, caps, &|p, c| {
                    if p == pos {
                        return false;
                    }
                    self.match_repeat(node, min, max, greedy, p, ctx, c, count + 1, cont)
                });
            }
            false
        }
    }

    /// 字符等值判断（`i` 标志下做 ASCII/Unicode 简单大小写折叠）。
    fn char_eq(&self, a: Option<char>, b: char) -> bool {
        let Some(a) = a else { return false };
        if self.ignore_case {
            a.to_lowercase().eq(b.to_lowercase())
        } else {
            a == b
        }
    }

    /// 字符类成员判断。
    fn class_item_matches(&self, item: &ClassItem, ch: char) -> bool {
        match item {
            ClassItem::Ch(c) => self.char_eq(Some(ch), *c),
            ClassItem::Range(lo, hi) => {
                if self.ignore_case {
                    let lc = ch.to_lowercase().next().unwrap_or(ch);
                    let uc = ch.to_uppercase().next().unwrap_or(ch);
                    (*lo..=*hi).contains(&lc) || (*lo..=*hi).contains(&uc)
                } else {
                    (*lo..=*hi).contains(&ch)
                }
            }
            ClassItem::Digit => ch.is_ascii_digit(),
            ClassItem::NotDigit => !ch.is_ascii_digit(),
            ClassItem::Word => ch.is_alphanumeric() || ch == '_',
            ClassItem::NotWord => !(ch.is_alphanumeric() || ch == '_'),
            ClassItem::Space => ch.is_whitespace(),
            ClassItem::NotSpace => !ch.is_whitespace(),
        }
    }
}

/// 匹配上下文：输入与回溯预算（计数用 `Cell`，全程共享借用以便闭包捕获）。
struct MatchCtx<'a> {
    input: &'a [char],
    steps: std::cell::Cell<u32>,
    exceeded: std::cell::Cell<bool>,
}

impl<'a> MatchCtx<'a> {
    /// 消耗一步预算；耗尽时置位 `exceeded`（层层短路回退，外层转换为错误）。
    fn step(&self) {
        if self.steps.get() == 0 {
            self.exceeded.set(true);
            return;
        }
        self.steps.set(self.steps.get() - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_pattern_extracts_order_match() {
        let re = Regex::compile(r"([a-z]+)-(\d+)", "i").expect("compile");
        let m = re.find("Order-123").expect("run").expect("should match");
        assert_eq!((m.start, m.end), (0, 9));
        let text = "Order-123";
        let full: String = text.chars().collect::<Vec<_>>()[m.start..m.end]
            .iter()
            .collect();
        assert_eq!(full, "Order-123");
        let (g1s, g1e) = m.groups[0].expect("group1");
        let g1: String = text.chars().collect::<Vec<_>>()[g1s..g1e].iter().collect();
        assert_eq!(g1, "Order");
        let (g2s, g2e) = m.groups[1].expect("group2");
        let g2: String = text.chars().collect::<Vec<_>>()[g2s..g2e].iter().collect();
        assert_eq!(g2, "123");
    }

    #[test]
    fn no_match_returns_none() {
        let re = Regex::compile(r"\d+", "").expect("compile");
        assert!(re.find("abc").expect("run").is_none());
        assert!(!re.test("abc").expect("run"));
    }

    #[test]
    fn alternation_and_anchors() {
        let re = Regex::compile("^(cat|dog)$", "").expect("compile");
        assert!(re.test("cat").expect("run"));
        assert!(re.test("dog").expect("run"));
        assert!(!re.test("cow").expect("run"));
        assert!(!re.test("catdog").expect("run"));
    }

    #[test]
    fn counted_and_lazy_quantifiers() {
        let re = Regex::compile("a{2,3}", "").expect("compile");
        let m = re.find("aaaa").expect("run").expect("match");
        assert_eq!((m.start, m.end), (0, 3), "贪婪取 3 次");
        let lazy = Regex::compile(r"a+?b", "").expect("compile");
        let m = lazy.find("aaab").expect("run").expect("match");
        assert_eq!((m.start, m.end), (0, 4));
    }

    #[test]
    fn negated_class_and_dot() {
        let re = Regex::compile(r"[^0-9]+", "").expect("compile");
        let m = re.find("12ab34").expect("run").expect("match");
        assert_eq!((m.start, m.end), (2, 4));
        let dot = Regex::compile("a.c", "").expect("compile");
        assert!(dot.test("abc").expect("run"));
    }

    #[test]
    fn ignore_case_flag() {
        let re = Regex::compile("hello", "i").expect("compile");
        assert!(re.test("HeLLo").expect("run"));
        let cs = Regex::compile("hello", "").expect("compile");
        assert!(!cs.test("HeLLo").expect("run"));
    }

    #[test]
    fn syntax_error_is_reported() {
        assert!(matches!(
            Regex::compile("(unclosed", ""),
            Err(RegexError::Syntax(_))
        ));
        assert!(matches!(
            Regex::compile("[z-a]", ""),
            Err(RegexError::Syntax(_))
        ));
    }

    #[test]
    fn multiline_flag_matches_line_anchors() {
        let plain = Regex::compile("^b", "").expect("compile");
        assert!(!plain.test("a\nb").expect("run"));
        let multi = Regex::compile("^b", "m").expect("compile");
        assert!(multi.test("a\nb").expect("run"));
        let end = Regex::compile("a$", "m").expect("compile");
        assert!(end.test("a\nb").expect("run"));
        let end_plain = Regex::compile("a$", "").expect("compile");
        assert!(!end_plain.test("a\nb").expect("run"));
    }

    #[test]
    fn named_groups_carry_names_and_stay_numbered() {
        let re = Regex::compile("(?<y>\\d{4})-(\\d{2})", "").expect("compile");
        assert_eq!(re.group_count(), 2);
        assert_eq!(re.group_names()[0].as_deref(), Some("y"));
        assert_eq!(re.group_names()[1], None);
        let m = re.find("2026-08").expect("run").expect("match");
        assert_eq!(m.groups.len(), 2);
    }

    #[test]
    fn find_at_honors_start_offset() {
        let re = Regex::compile(r"\d+", "").expect("compile");
        let first = re.find("a1 b22").expect("run").expect("match");
        assert_eq!((first.start, first.end), (1, 2));
        let second = re
            .find_at("a1 b22", first.end)
            .expect("run")
            .expect("match");
        assert_eq!((second.start, second.end), (4, 6));
        assert!(re.find_at("a1 b22", 6).expect("run").is_none());
    }

    #[test]
    fn lookbehind_still_unsupported() {
        assert!(matches!(
            Regex::compile("(?<=a)b", ""),
            Err(RegexError::Syntax(_))
        ));
    }
}
