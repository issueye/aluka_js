//! 隐藏类（Shape）：对象布局的共享描述。
//!
//! 结构相同的对象共享同一个 [`Shape`]，属性访问因此可以退化为"校验
//! shape 相同 → 直接读固定槽位"，这是内联缓存（IC）的基础。Shape 沿
//! transition 树增长：在既有 shape 上添加属性得到子 shape，前缀相同的
//! 对象自动共享祖先。
//!
//! 删除属性不改 shape（否则会污染共享），而是在对象级记录；这一点与
//! Go 版一致，见 `internal/engine/shape.go` 的同款设计。

use std::collections::HashMap;

/// Shape 的全局唯一标识。IC 用它做一次整数比较来判断"对象布局是否仍是
/// 缓存记录的那个"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShapeId(pub u32);

/// 一组属性名到槽位下标的映射。
///
/// `names` 保持插入顺序（即槽位顺序），`index` 提供 O(1) 名字查找。
/// Shape 不可变：任何结构变更都派生新的 Shape，因此可以安全共享。
#[derive(Debug)]
pub struct Shape {
    id: ShapeId,
    names: Vec<String>,
    index: HashMap<String, usize>,
}

impl Shape {
    /// 创建空 shape（无属性），作为 transition 树的根。
    #[must_use]
    pub fn root(id: ShapeId) -> Self {
        Self {
            id,
            names: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// 该 shape 的唯一标识。
    #[must_use]
    pub fn id(&self) -> ShapeId {
        self.id
    }

    /// 属性数量，即对象需要的槽位数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// 是否没有任何属性。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// 查属性名对应的槽位下标。
    #[must_use]
    pub fn lookup(&self, name: &str) -> Option<usize> {
        self.index.get(name).copied()
    }

    /// 按槽位顺序遍历属性名（`Object.keys` 的枚举顺序基础）。
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.names.iter().map(String::as_str)
    }

    /// 派生"在本 shape 上追加 `name`"的新 shape。
    ///
    /// 一般不直接调用：transition 缓存由 [`ShapeTable`] 统一管理（同一
    /// (shape, name) 复用同一子 shape），直接派生会让对象独占 shape、IC 全部失效。
    #[must_use]
    pub fn extend(&self, id: ShapeId, name: &str) -> Self {
        let mut names = self.names.clone();
        let mut index = self.index.clone();
        index.insert(name.to_owned(), names.len());
        names.push(name.to_owned());
        Self { id, names, index }
    }
}

/// Shape 的 transition 树缓存：`(父 ShapeId, 属性名) -> 子 Shape` 的共享表。
///
/// [`Shape::extend`] 本身是无状态的派生；本表负责复用派生结果——同一
/// (shape, name) 只产生一个子 shape，保证结构相同的对象收敛到同一
/// [`ShapeId`]（内联缓存命中的前提）。id 单调分配，树随程序运行增长。
///
/// 内部以 `Vec` 按下标存储（`shapes[id.0]`），`shape(id)` 为 O(1)——热点
/// 属性访问（隐藏类查找）依赖这一点，不做线性扫描。
#[derive(Debug)]
pub struct ShapeTable {
    next_id: u32,
    /// 按下标存 shape：`shapes[id.0]`，根为 `shapes[0]`。
    shapes: Vec<Shape>,
    /// transition 缓存：(父 id, 属性名) → 已复用的子 shape id。
    transitions: HashMap<(ShapeId, String), ShapeId>,
}

impl Default for ShapeTable {
    /// 等价于 [`ShapeTable::new`]（clippy::new_without_default 配对要求）。
    fn default() -> Self {
        Self::new()
    }
}

impl ShapeTable {
    /// 创建只含根 shape 的表。
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_id: 1,
            shapes: vec![Shape::root(ShapeId(0))],
            transitions: HashMap::new(),
        }
    }

    /// 根 shape（所有对象的初始布局）。
    #[must_use]
    pub fn root(&self) -> &Shape {
        &self.shapes[0]
    }

    /// 表内 shape 总数（含根；含形状数上限统计用）。
    #[must_use]
    pub fn len(&self) -> usize {
        self.shapes.len()
    }

    /// 表是否为空（仅含根 shape 即视为非空，恒为假）。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    /// 按 id 查询 shape（供属性 lookup 与调试）。O(1) 下标访问。
    #[must_use]
    pub fn shape(&self, id: ShapeId) -> Option<&Shape> {
        self.shapes.get(id.0 as usize)
    }

    /// 在 `from` shape 上追加 `name`，返回子 shape 的 id：命中缓存则复用，
    /// 否则派生并登记。调用方只持有 id（IC 比较与槽位查询均够用）。
    pub fn transition(&mut self, from: ShapeId, name: &str) -> ShapeId {
        let key = (from, name.to_owned());
        if let Some(&existing) = self.transitions.get(&key) {
            return existing;
        }
        let child_id = ShapeId(self.next_id);
        let child = match self.shapes.get(from.0 as usize) {
            Some(parent) => parent.extend(child_id, name),
            None => return from, // 未知 shape：原样返回（调用方契约违反，不 panic）
        };
        self.next_id += 1;
        debug_assert_eq!(child_id.0 as usize, self.shapes.len(), "id 必须与下标连续");
        self.shapes.push(child);
        self.transitions.insert(key, child_id);
        child_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_shape_is_empty() {
        let root = Shape::root(ShapeId(1));
        assert!(root.is_empty());
        assert_eq!(root.len(), 0);
        assert_eq!(root.lookup("a"), None);
    }

    #[test]
    fn extend_assigns_sequential_slots_and_keeps_order() {
        let root = Shape::root(ShapeId(1));
        let with_a = root.extend(ShapeId(2), "a");
        let with_ab = with_a.extend(ShapeId(3), "b");

        assert_eq!(with_ab.lookup("a"), Some(0));
        assert_eq!(with_ab.lookup("b"), Some(1));
        assert_eq!(with_ab.len(), 2);
        assert_eq!(with_ab.names().collect::<Vec<_>>(), vec!["a", "b"]);

        // 派生不改动父 shape：共享前缀的对象仍能命中祖先。
        assert_eq!(with_a.len(), 1);
        assert_eq!(root.len(), 0);
    }
}

#[cfg(test)]
mod shape_table_tests {
    use super::*;

    #[test]
    fn same_transition_reuses_child_shape() {
        let mut table = ShapeTable::new();
        let root_id = table.root().id();

        let first = table.transition(root_id, "x");
        let second = table.transition(root_id, "x");
        assert_eq!(first, second, "同一 (shape, name) 必须复用同一子 shape");

        let other = table.transition(root_id, "y");
        assert_ne!(first, other);
    }

    #[test]
    fn chained_transitions_form_a_tree() {
        let mut table = ShapeTable::new();
        let root_id = table.root().id();
        let with_a = table.transition(root_id, "a");
        let with_ab = table.transition(with_a, "b");

        let ab = table.shape(with_ab).expect("id 应可查询");
        assert_eq!(ab.lookup("a"), Some(0));
        assert_eq!(ab.lookup("b"), Some(1));
        // 同一派生请求幂等
        assert_eq!(with_ab, table.transition(with_a, "b"));
        // 根 shape 本身也可查询
        assert!(table.shape(root_id).is_some());
    }
}
