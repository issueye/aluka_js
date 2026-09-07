# aluvm ISA 字节码规范（aluvm ISA Specification）

> **状态**：正式规范（v1.0）｜**对应实现**：`ALUKABC1` FormatVersion 30
> **定位**：aluka 前端（alukac）与后端（aluvm）之间的**唯一跨语言、跨实现接口契约**。
> 本文是第三方独立实现前端编译器或后端虚拟机/JIT 的权威依据。

---

## 1. 架构总览与执行模型

aluvm 是一台基于操作数栈的虚拟机（Stack-based VM），采用定长 4 字节指令格式。

### 1.1 核心执行模型
- **操作数栈（Value Stack）**：绝大多数算术、逻辑、属性访问及函数调用参数均通过栈进行评估。
- **局部槽位（Locals）**：存储当前执行帧的局部变量与参数，按 `slot` 索引直接访问（0 为首个局部变量或 `this`）。
- **词法作用域闭包（Upvalues）**：跨层捕获的变量通过 Upvalue 链表或平铺槽位进行读写；函数退出或循环变量复用前显式执行 `CLOSE_UPVALUES`。
- **常量池（Constant Pool）**：重值（浮点数、大整数、字符串、类与函数模板）统一存放在函数常量池中，通过索引访问。

---

## 2. 二进制文件格式与序列化布局

aluka 编译生成的独立字节码文件或磁盘缓存模块使用 `ALUKABC1` 容器格式。

### 2.1 字节序规则（重要）
> [!WARNING]
> **全文件小端序，唯指令操作数大端序**：
> - 容器头、长度字段、浮点数常量、Try 表全部为 **小端序（Little-Endian）**。
> - 唯独指令体中的 3 字节操作数域为 **大端序（Big-Endian uint24）**。

### 2.2 顶层容器布局

```
+-----------------------------------------------------------------------+
| Magic: "ALUKABC1" (8 字节 ASCII)                                      |
+-----------------------------------+-----------------------------------+
| FormatVersion: u32 (当前为 30)     | FunctionsCount: u32               |
+-----------------------------------+-----------------------------------+
| ClassesCount: u32                 |                                   |
+-----------------------------------+-----------------------------------+
| Functions... (依次排布 FunctionsCount 个 FuncTemplate)                |
+-----------------------------------------------------------------------+
| Classes... (依次排布 ClassesCount 个 ClassTemplate)                    |
+-----------------------------------------------------------------------+
```


### 2.3 函数模板（FuncTemplate）布局

每个函数模板依次包含以下内容（全为小端标量）：

1. **函数名（Name）**：`len: u32` + UTF-8 字节串。
2. **标量头（13 个 u32 字段，共 52 字节）**：
   - `NumParams: u32`：形参数量。
   - `NumLocals: u32`：分配的局部变量槽位数（必须 `>= NumParams`）。
   - `IsVarArgs: u32`（0 或 1）：是否包含 Rest 参数。
   - `IsGenerator: u32`（0 或 1）：是否为生成器函数。
   - `IsAsync: u32`（0 或 1）：是否为 async 函数。
   - `IsArrow: u32`（0 或 1）：是否为箭头函数。
   - `CodeLen: u32`：指令字节数（必须为 4 的整数倍）。
   - `ArgumentsSlot: int32 (存储为 u32)`：arguments 对象绑定的槽位；若为 `-1` 表示无 arguments。
   - `NoArgumentsObject: u32`（0 或 1）：是否显式省略 arguments 对象。
   - `NewTargetSlot: int32 (存储为 u32)`：new.target 存储槽位；`-1` 表示无。
   - `Inlinable: u32`（0 或 1）：优化器标记是否可内联。
   - `NFESlot: int32 (存储为 u32)`：具名函数表达式自身引用的存储槽位；`-1` 表示无。
   - `MaxStack: u32`：操作数栈最大可能深度。
3. **指令体（Code）**：`CodeLen` 字节的字节码流。
4. **源文件名（SourceFile）**：`len: u32` + UTF-8 字节串。
5. **常量池（Constants）**：
   - `Count: u32`
   - 逐个常量项编码：
     - `Tag: u8 = 1 (Number)`：紧跟 8 字节小端 IEEE-754 `float64`。
     - `Tag: u8 = 2 (String)`：紧跟 `len: u32` + UTF-8 字节串。
     - `Tag: u8 = 3 (BigInt)`：紧跟 `len: u32` + 十进制 ASCII 字符串（如 `"12345678901234567890"`）。
     - `Tag: u8 = 4 (Bool)`：紧跟 1 字节（`0` 表示 false，`1` 表示 true）。
     - `Tag: u8 = 5 (Null)`：无载荷。
6. **上值捕获表（Upvalues）**：
   - `Count: u32`
   - 每项：`IsLocal: u32`（1 表示捕获直接外层局部槽位，0 表示捕获外层的 Upvalue）+ `Index: u32`（外层槽位或外层 Upvalue 索引）。
7. **原生回调扩展描述（NativeCallback）**：
   - `HasNative: u32`（0 表示无；1 表示有描述体）。
8. **异常与清理表（TryTable）**：
   - `Count: u32`
   - 每项 8 个 u32（32 字节）：
     - `StartPC: u32`：保护区域起始指令偏移（包含）。
     - `CatchPC: u32`：Catch 处理入口偏移。
     - `FinallyPC: u32`：Finally 处理入口偏移。
     - `HasCatch: u32`（0 或 1）
     - `HasFinally: u32`（0 或 1）
     - `EndPC: u32`：保护区域结束指令偏移（不包含）。
     - `CatchEndPC: u32`：Catch 区域结束指令偏移。
     - `FinallyEndPC: u32`：Finally 区域结束指令偏移。
9. **行号调试映射（LineStarts）**：
   - `Count: u32`
   - 每项：`PC: u32` + `Line: u32`（源文件行号）。

---

## 3. 编码陷阱与实现者必读

任何实现者在构建前端编译器或后端解释器时，必须严格遵守以下约定：

1. **指令操作数大端序 vs 序列化小端序**：
   - 指令解码：`opcode = code[pc]`, `operand = (code[pc+1]<<16) | (code[pc+2]<<8) | code[pc+3]`。
   - 文件序列化字段（如常量池长度、版本号）全为小端序。
2. **双字段打包操作数**：
   - `OpGetPropLocal`（100）：高 16 位为局部变量槽位 `slot = operand >> 16`；低 16 位为属性名常量池下标 `nameIdx = operand & 0xFFFF`。
   - `OpCallMethod`（50）：高 16 位为传参数量 `numArgs = operand >> 16`；低 16 位为方法名常量池下标 `nameIdx = operand & 0xFFFF`。
3. **跳转边界语义**：
   - 所有跳转操作数（`OperandSignedOff`）为相对偏移（相对于**下一条指令的起始 PC**）。
   - 跳转目标允许 `target == len(code)`：虚拟机视其为合法的控制流终点，隐式执行 `return undefined`。这由规范强制保证，不得判为越界。
4. **负整数与哨兵槽位的符号转换**：
   - `ArgumentsSlot`、`NewTargetSlot`、`NFESlot` 字段在二进制中写为 32 位无符号整数，但在语义上是带符号整数。反序列化时必须按 `int32` 解释，`-1`（`0xFFFFFFFF`）代表未设置。若错当成正数将导致运行期数组越界 panic。
5. **BigInt 十进制字符串存储**：
   - 字节码常量池中的 BigInt 并非二进制补码存储，而是保存为以 10 为基数的明文字符串。
6. **未分发指令与哨兵**：
   - `OpForInNext`（84）：历史遗留指令，虚拟机分发循环中没有实现该 case；新代码统一由 `OpEnumKeys` 处理。
   - `OpEnd`（105）：代码结尾的安全哨兵指令，不应当在正常执行流中被解释运行。

---

## 4. 11 种操作数类别规范（OperandKind）

| 操作数类别 | 数值 | 宽度 | 语义与有效范围 | 举例指令 |
|-----------|------|------|----------------|----------|
| `OperandNone` | 0 | 0（占位0） | 不读取 3 字节操作数域，执行时忽略 | NOP, ADD, POP, RETURN |
| `OperandConstIdx` | 1 | 3 字节 uint24 | 常量池索引 [0, len(Constants)-1] | PUSH_CONST, LOAD_GLOBAL, GET_PROP |
| `OperandInt` | 2 | 3 字节 uint24 | 24 位无符号内联整数 [0, 16777215] | PUSH_INT, PUSH_NEG_INT |
| `OperandSlot` | 3 | 3 字节 uint24 | 当前调用帧局部槽位 [0, NumLocals-1] | LOAD_LOCAL, STORE_LOCAL, CLOSE_UPVALUES |
| `OperandUpvalueIdx` | 4 | 3 字节 uint24 | 当前函数闭包捕获表索引 [0, len(Upvalues)-1] | LOAD_UPVALUE, STORE_UPVALUE |
| `OperandTemplateIdx` | 5 | 3 字节 uint24 | 模块级函数/类模板索引 [0, len(Templates)-1] | MAKE_CLOSURE, MAKE_CLASS |
| `OperandTryIdx` | 6 | 3 字节 uint24 | 当前函数 TryTable 索引 [0, len(TryTable)-1] | TRY_ENTER, TRY_EXIT, TRY_EXIT_FINALLY |
| `OperandSignedOff` | 7 | 3 字节 int24 | 有符号相对跳转字节偏移（24 位补码，范围 ±8MB） | JMP, JMP_TRUE_POP, TRY_EXIT_JMP |
| `OperandCount` | 8 | 3 字节 uint24 | 参数数量或元素数量 | CALL, NEW, NEW_ARRAY, NEW_OBJECT |
| `OperandPackedSlotName` | 9 | 3 字节 (16+16) | 打包槽位 slot = op>>16 与常量名 nameIdx = op&0xFFFF | GET_PROP_LOCAL |
| `OperandPackedCall` | 10 | 3 字节 (16+16) | 打包实参 numArgs = op>>16 与常量名 nameIdx = op&0xFFFF | CALL_METHOD |

---

## 5. 106 条指令全集规范表（由单一事实源导出）

| 操作码 | 常量名 | 指令名 | 操作数类别 | 出栈 | 入栈 | 净栈变动 | 纯压栈 | 跳转 | 终结 | 条件/动态栈 | 说明与陷阱 |
|:---:|---|---|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|---|
| 0 | `OpNop` | `NOP` | `OperandNone` | 0 | 0 | `+0` | 否 | 否 | 否 | 固定 |  |
| 1 | `OpPushUndefined` | `PUSH_UNDEFINED` | `OperandNone` | 0 | 1 | `+1` | 是 | 否 | 否 | 固定 |  |
| 2 | `OpPushNull` | `PUSH_NULL` | `OperandNone` | 0 | 1 | `+1` | 是 | 否 | 否 | 固定 |  |
| 3 | `OpPushTrue` | `PUSH_TRUE` | `OperandNone` | 0 | 1 | `+1` | 是 | 否 | 否 | 固定 |  |
| 4 | `OpPushFalse` | `PUSH_FALSE` | `OperandNone` | 0 | 1 | `+1` | 是 | 否 | 否 | 固定 |  |
| 5 | `OpPushConst` | `PUSH_CONST` | `OperandConstIdx` | 0 | 1 | `+1` | 是 | 否 | 否 | 固定 |  |
| 6 | `OpPushInt` | `PUSH_INT` | `OperandInt` | 0 | 1 | `+1` | 是 | 否 | 否 | 固定 | 立即数：24位无符号小整数(0..16777215) |
| 7 | `OpPushNegInt` | `PUSH_NEG_INT` | `OperandInt` | 0 | 1 | `+1` | 是 | 否 | 否 | 固定 | 立即数：24位负整数，压入 -(A) |
| 8 | `OpPop` | `POP` | `OperandNone` | 1 | 0 | `-1` | 否 | 否 | 否 | 固定 |  |
| 9 | `OpDup` | `DUP` | `OperandNone` | 0 | 1 | `+1` | 否 | 否 | 否 | 固定 |  |
| 10 | `OpSwap` | `SWAP` | `OperandNone` | 2 | 2 | `+0` | 否 | 否 | 否 | 固定 |  |
| 11 | `OpLoadLocal` | `LOAD_LOCAL` | `OperandSlot` | 0 | 1 | `+1` | 否 | 否 | 否 | 固定 |  |
| 12 | `OpStoreLocal` | `STORE_LOCAL` | `OperandSlot` | 1 | 0 | `-1` | 否 | 否 | 否 | 固定 |  |
| 13 | `OpLoadGlobal` | `LOAD_GLOBAL` | `OperandConstIdx` | 0 | 1 | `+1` | 否 | 否 | 否 | 固定 |  |
| 14 | `OpStoreGlobal` | `STORE_GLOBAL` | `OperandConstIdx` | 1 | 0 | `-1` | 否 | 否 | 否 | 固定 |  |
| 15 | `OpLoadUpvalue` | `LOAD_UPVALUE` | `OperandUpvalueIdx` | 0 | 1 | `+1` | 否 | 否 | 否 | 固定 |  |
| 16 | `OpStoreUpvalue` | `STORE_UPVALUE` | `OperandUpvalueIdx` | 1 | 0 | `-1` | 否 | 否 | 否 | 固定 |  |
| 17 | `OpMakeClosure` | `MAKE_CLOSURE` | `OperandTemplateIdx` | 0 | 1 | `+1` | 否 | 否 | 否 | 固定 |  |
| 18 | `OpAdd` | `ADD` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 19 | `OpSub` | `SUB` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 20 | `OpMul` | `MUL` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 21 | `OpDiv` | `DIV` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 22 | `OpMod` | `MOD` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 23 | `OpPow` | `POW` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 24 | `OpBitAnd` | `BIT_AND` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 25 | `OpBitOr` | `BIT_OR` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 26 | `OpBitXor` | `BIT_XOR` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 27 | `OpShl` | `SHL` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 28 | `OpShr` | `SHR` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 29 | `OpUShr` | `USHR` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 30 | `OpNeg` | `NEG` | `OperandNone` | 1 | 1 | `+0` | 否 | 否 | 否 | 固定 |  |
| 31 | `OpNot` | `NOT` | `OperandNone` | 1 | 1 | `+0` | 否 | 否 | 否 | 固定 |  |
| 32 | `OpBitNot` | `BIT_NOT` | `OperandNone` | 1 | 1 | `+0` | 否 | 否 | 否 | 固定 |  |
| 33 | `OpTypeof` | `TYPEOF` | `OperandNone` | 1 | 1 | `+0` | 否 | 否 | 否 | 固定 |  |
| 34 | `OpEq` | `EQ` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 35 | `OpStrictEq` | `STRICT_EQ` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 36 | `OpStrictNe` | `STRICT_NE` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 37 | `OpNe` | `NE` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 38 | `OpLt` | `LT` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 39 | `OpLe` | `LE` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 40 | `OpGt` | `GT` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 41 | `OpGe` | `GE` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 42 | `OpJmp` | `JMP` | `OperandSignedOff` | 0 | 0 | `+0` | 否 | 是 | 否 | 固定 | 相对跳转；跳转目标允许 target == len(code) |
| 43 | `OpJmpTruePop` | `JMP_TRUE_POP` | `OperandSignedOff` | 1 | 0 | `-1` | 否 | 是 | 否 | 固定 |  |
| 44 | `OpJmpFalsePop` | `JMP_FALSE_POP` | `OperandSignedOff` | 1 | 0 | `-1` | 否 | 是 | 否 | 固定 |  |
| 45 | `OpJmpTrueKeep` | `JMP_TRUE_KEEP` | `OperandSignedOff` | 0 | 0 | `动态` | 否 | 是 | 否 | 条件栈变 | 相对跳转；跳转目标允许 target == len(code) |
| 46 | `OpJmpFalseKeep` | `JMP_FALSE_KEEP` | `OperandSignedOff` | 0 | 0 | `动态` | 否 | 是 | 否 | 条件栈变 | 相对跳转；跳转目标允许 target == len(code) |
| 47 | `OpJmpNullishKeep` | `JMP_NULLISH_KEEP` | `OperandSignedOff` | 0 | 0 | `动态` | 否 | 是 | 否 | 条件栈变 | 相对跳转；跳转目标允许 target == len(code) |
| 48 | `OpOptionalJump` | `OPTIONAL_JUMP` | `OperandSignedOff` | 0 | 0 | `动态` | 否 | 是 | 否 | 条件栈变 |  |
| 49 | `OpCall` | `CALL` | `OperandCount` | 0 | 1 | `动态` | 否 | 否 | 否 | 动态栈变 |  |
| 50 | `OpCallMethod` | `CALL_METHOD` | `OperandPackedCall` | 0 | 1 | `动态` | 否 | 否 | 否 | 动态栈变 | 打包调用：参数数量与方法名(numArgs<<16 | nameIdx) |
| 51 | `OpCallWithThis` | `CALL_WITH_THIS` | `OperandCount` | 0 | 1 | `动态` | 否 | 否 | 否 | 动态栈变 |  |
| 52 | `OpCallWithThisArgs` | `CALL_WITH_THIS_ARGS` | `OperandNone` | 0 | 1 | `动态` | 否 | 否 | 否 | 动态栈变 |  |
| 53 | `OpNew` | `NEW` | `OperandCount` | 0 | 1 | `动态` | 否 | 否 | 否 | 动态栈变 |  |
| 54 | `OpReturn` | `RETURN` | `OperandNone` | 1 | 0 | `-1` | 否 | 否 | 是 | 固定 |  |
| 55 | `OpReturnUndef` | `RETURN_UNDEF` | `OperandNone` | 0 | 0 | `+0` | 否 | 否 | 是 | 固定 |  |
| 56 | `OpNewObject` | `NEW_OBJECT` | `OperandCount` | 0 | 1 | `动态` | 否 | 否 | 否 | 动态栈变 |  |
| 57 | `OpNewArray` | `NEW_ARRAY` | `OperandCount` | 0 | 1 | `动态` | 否 | 否 | 否 | 动态栈变 |  |
| 58 | `OpGetProp` | `GET_PROP` | `OperandConstIdx` | 1 | 1 | `+0` | 否 | 否 | 否 | 固定 |  |
| 59 | `OpSetProp` | `SET_PROP` | `OperandConstIdx` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 60 | `OpSetPropObj` | `SET_PROP_OBJ` | `OperandConstIdx` | 1 | 0 | `-1` | 否 | 否 | 否 | 固定 |  |
| 61 | `OpSetPropTop` | `SET_PROP_TOP` | `OperandConstIdx` | 2 | 0 | `-2` | 否 | 否 | 否 | 固定 |  |
| 62 | `OpGetElem` | `GET_ELEM` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 63 | `OpSetElem` | `SET_ELEM` | `OperandNone` | 3 | 1 | `-2` | 否 | 否 | 否 | 固定 |  |
| 64 | `OpSetElemTop` | `SET_ELEM_TOP` | `OperandNone` | 3 | 0 | `-3` | 否 | 否 | 否 | 固定 |  |
| 65 | `OpDelProp` | `DEL_PROP` | `OperandConstIdx` | 1 | 1 | `+0` | 否 | 否 | 否 | 固定 |  |
| 66 | `OpDelElem` | `DEL_ELEM` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 67 | `OpSetPropComputedObj` | `SET_PROP_COMPUTED_OBJ` | `OperandNone` | 2 | 0 | `-2` | 否 | 否 | 否 | 固定 |  |
| 68 | `OpBuildArray` | `BUILD_ARRAY` | `OperandNone` | 0 | 1 | `+1` | 否 | 否 | 否 | 固定 |  |
| 69 | `OpArrayPush` | `ARRAY_PUSH` | `OperandNone` | 1 | 0 | `-1` | 否 | 否 | 否 | 固定 |  |
| 70 | `OpArraySpread` | `ARRAY_SPREAD` | `OperandNone` | 1 | 0 | `-1` | 否 | 否 | 否 | 固定 |  |
| 71 | `OpCallArgs` | `CALL_ARGS` | `OperandNone` | 0 | 1 | `动态` | 否 | 否 | 否 | 动态栈变 |  |
| 72 | `OpCallMethodArgs` | `CALL_METHOD_ARGS` | `OperandConstIdx` | 0 | 1 | `动态` | 否 | 否 | 否 | 动态栈变 |  |
| 73 | `OpNewArgs` | `NEW_ARGS` | `OperandNone` | 0 | 1 | `动态` | 否 | 否 | 否 | 动态栈变 |  |
| 74 | `OpSpreadObject` | `SPREAD_OBJECT` | `OperandNone` | 1 | 0 | `-1` | 否 | 否 | 否 | 固定 |  |
| 75 | `OpUnaryPlus` | `UNARY_PLUS` | `OperandNone` | 1 | 1 | `+0` | 否 | 否 | 否 | 固定 |  |
| 76 | `OpTypeofGlobal` | `TYPEOF_GLOBAL` | `OperandConstIdx` | 0 | 1 | `+1` | 否 | 否 | 否 | 固定 |  |
| 77 | `OpTryEnter` | `TRY_ENTER` | `OperandTryIdx` | 0 | 0 | `+0` | 否 | 否 | 否 | 固定 |  |
| 78 | `OpTryExit` | `TRY_EXIT` | `OperandTryIdx` | 0 | 0 | `+0` | 否 | 否 | 否 | 固定 |  |
| 79 | `OpTryExitFinally` | `TRY_EXIT_FINALLY` | `OperandTryIdx` | 0 | 0 | `+0` | 否 | 否 | 否 | 固定 |  |
| 80 | `OpTryExitJmp` | `TRY_EXIT_JMP` | `OperandSignedOff` | 0 | 0 | `+0` | 否 | 是 | 否 | 固定 |  |
| 81 | `OpThrow` | `THROW` | `OperandNone` | 1 | 0 | `-1` | 否 | 否 | 是 | 固定 |  |
| 82 | `OpInstanceof` | `INSTANCEOF` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 83 | `OpIn` | `IN` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 |  |
| 84 | `OpForInNext` | `FOR_IN_NEXT` | `OperandSignedOff` | 0 | 0 | `动态` | 否 | 是 | 否 | 动态栈变 | 遗留指令；VM中无对应dispatch分支 |
| 85 | `OpMakeClass` | `MAKE_CLASS` | `OperandTemplateIdx` | 0 | 1 | `动态` | 否 | 否 | 否 | 动态栈变 |  |
| 86 | `OpGetProto` | `GET_PROTO` | `OperandNone` | 1 | 1 | `+0` | 否 | 否 | 否 | 固定 |  |
| 87 | `OpCallThis` | `CALL_THIS` | `OperandCount` | 0 | 1 | `动态` | 否 | 否 | 否 | 动态栈变 |  |
| 88 | `OpConstructThis` | `CONSTRUCT_THIS` | `OperandCount` | 0 | 1 | `动态` | 否 | 否 | 否 | 动态栈变 |  |
| 89 | `OpCallThisArgs` | `CALL_THIS_ARGS` | `OperandNone` | 0 | 1 | `动态` | 否 | 否 | 否 | 动态栈变 |  |
| 90 | `OpConstructThisArgs` | `CONSTRUCT_THIS_ARGS` | `OperandNone` | 0 | 1 | `动态` | 否 | 否 | 否 | 动态栈变 |  |
| 91 | `OpGetIterator` | `GET_ITERATOR` | `OperandNone` | 1 | 1 | `+0` | 否 | 否 | 否 | 固定 |  |
| 92 | `OpYield` | `YIELD` | `OperandNone` | 1 | 0 | `-1` | 否 | 否 | 否 | 固定 |  |
| 93 | `OpGetAsyncIterator` | `GET_ASYNC_ITERATOR` | `OperandNone` | 1 | 1 | `+0` | 否 | 否 | 否 | 固定 |  |
| 94 | `OpAwait` | `AWAIT` | `OperandNone` | 1 | 1 | `+0` | 否 | 否 | 否 | 固定 |  |
| 95 | `OpMakeRegexp` | `MAKE_REGEXP` | `OperandNone` | 2 | 1 | `-1` | 否 | 否 | 否 | 固定 | 弹栈flags+pattern，生成RegExp对象 |
| 96 | `OpSetGetterObj` | `SET_GETTER_OBJ` | `OperandConstIdx` | 1 | 0 | `-1` | 否 | 否 | 否 | 固定 |  |
| 97 | `OpSetSetterObj` | `SET_SETTER_OBJ` | `OperandConstIdx` | 1 | 0 | `-1` | 否 | 否 | 否 | 固定 |  |
| 98 | `OpSetGetterComputedObj` | `SET_GETTER_COMPUTED_OBJ` | `OperandNone` | 2 | 0 | `-2` | 否 | 否 | 否 | 固定 |  |
| 99 | `OpSetSetterComputedObj` | `SET_SETTER_COMPUTED_OBJ` | `OperandNone` | 2 | 0 | `-2` | 否 | 否 | 否 | 固定 |  |
| 100 | `OpGetPropLocal` | `GET_PROP_LOCAL` | `OperandPackedSlotName` | 0 | 1 | `+1` | 否 | 否 | 否 | 固定 | 超指令：打包局部槽位与属性名(slot<<16 | nameIdx) |
| 101 | `OpCloseUpvalues` | `CLOSE_UPVALUES` | `OperandSlot` | 0 | 0 | `+0` | 否 | 否 | 否 | 固定 | 闭包作用域：关闭从指定槽位开始的捕获上值 |
| 102 | `OpInc` | `INC` | `OperandNone` | 1 | 1 | `+0` | 否 | 否 | 否 | 固定 |  |
| 103 | `OpDec` | `DEC` | `OperandNone` | 1 | 1 | `+0` | 否 | 否 | 否 | 固定 |  |
| 104 | `OpEnumKeys` | `ENUM_KEYS` | `OperandNone` | 1 | 1 | `+0` | 否 | 否 | 否 | 固定 | 弹栈对象，压入原型链可枚举键数组 |
| 105 | `OpEnd` | `END` | `OperandNone` | 0 | 0 | `+0` | 否 | 否 | 否 | 固定 | 代码结尾哨兵指令；不实际执行 |

---

## 6. 字节码校验器契约（Verifier Rules V1..V16）

为了确保字节码载入后的内存安全与执行健壮性，Rust verifier 必须强制执行以下 **通过即安全（Valid implies Safe）** 规则。

| 规则编号 | 校验维度 | 规则描述 | Go 侧现状 | Rust verifier 强度 |
|:---:|---|---|:---:|:---:|
| **V1** | 容器格式 | 校验文件魔数必须恒为 "ALUKABC1" | 已有 | 强制拒绝 |
| **V2** | 格式版本 | FormatVersion 必须与当前版本完全匹配（当前 30） | 已有 | 强制拒绝 |
| **V3** | 指令对齐 | len(Code) 必须为 4 的整数倍且指令 opcode 必须 <= OpEnd(105) | 已有 | 强制拒绝 |
| **V4** | 局部槽位越界 | 所有 OperandSlot 操作数必须 < NumLocals | 已有 | 强制拒绝 |
| **V5** | 常量池越界 | 所有 OperandConstIdx 操作数必须 < len(Constants) | 已有 | 强制拒绝 |
| **V6** | 常量池类型匹配 | 指令所需常量类型必须与池中条目类型严格匹配（如 LOAD_GLOBAL 必须为 String） | **缺失** | 强制校验类型 |
| **V7** | 跳转对齐与边界 | 相对跳转计算出的目标 PC 必须为 4 的倍数，且满足 0 <= target <= len(Code) | 已有 | 强制拒绝 |
| **V8** | 跨块栈深合流一致性 | 汇合基本块（有多个入边的目标 PC）无论从哪条分支到达，操作数栈深度必须绝对相等 | **缺失** | 深度优先/拓扑遍历强制断言 |
| **V9** | 栈下溢防范 | 任意指令执行前，当前栈深必须 >= Pops；整块执行完毕栈深必须 >= 0 | **缺失** | 静态单调推导拒绝对溢 |
| **V10** | 最大栈深约束 | 函数内任意程序点推导出的瞬时栈深不得超过 MaxStack | **缺失** | 验证 MaxStack 紧致性 |
| **V11** | TryTable 范围合法 | StartPC < EndPC 且两者均为 4 的倍数，位于 [0, len(Code)] 范围之内 | **缺失** | 强制校验 |
| **V12** | Handler 在 Body 之外 | CatchPC 和 FinallyPC 不得位于 [StartPC, EndPC) 区间内部 | **缺失** | 强制嵌套隔离 |
| **V13** | TryTable 嵌套合法性 | 两个 Try 区间若重叠，必须为严格的包含嵌套关系，禁止交叉重叠 | **缺失** | 区间嵌套断言 |
| **V14** | Try 边界字段校验 | CatchEndPC 与 FinallyEndPC 必须合规对齐且不越界 | **缺失** | 补齐 Go 侧遗漏 |
| **V15** | 模板与 Try 索引合法 | OperandTemplateIdx 与 OperandTryIdx 必须在模块级定义数组有效范围之内 | **缺失** | 模块级全局校验 |
| **V16** | 闭包捕获索引范围 | OperandUpvalueIdx 必须 < len(Upvalues)，且 Upvalues[i].Index 合法 | 部分已有 | 完整深度校验 |

---

## 7. 附录：106 条操作码快速索引表

```
[000] NOP                       [001] PUSH_UNDEFINED            [002] PUSH_NULL               
[003] PUSH_TRUE                 [004] PUSH_FALSE                [005] PUSH_CONST              
[006] PUSH_INT                  [007] PUSH_NEG_INT              [008] POP                     
[009] DUP                       [010] SWAP                      [011] LOAD_LOCAL              
[012] STORE_LOCAL               [013] LOAD_GLOBAL               [014] STORE_GLOBAL            
[015] LOAD_UPVALUE              [016] STORE_UPVALUE             [017] MAKE_CLOSURE            
[018] ADD                       [019] SUB                       [020] MUL                     
[021] DIV                       [022] MOD                       [023] POW                     
[024] BIT_AND                   [025] BIT_OR                    [026] BIT_XOR                 
[027] SHL                       [028] SHR                       [029] USHR                    
[030] NEG                       [031] NOT                       [032] BIT_NOT                 
[033] TYPEOF                    [034] EQ                        [035] STRICT_EQ               
[036] STRICT_NE                 [037] NE                        [038] LT                      
[039] LE                        [040] GT                        [041] GE                      
[042] JMP                       [043] JMP_TRUE_POP              [044] JMP_FALSE_POP           
[045] JMP_TRUE_KEEP             [046] JMP_FALSE_KEEP            [047] JMP_NULLISH_KEEP        
[048] OPTIONAL_JUMP             [049] CALL                      [050] CALL_METHOD             
[051] CALL_WITH_THIS            [052] CALL_WITH_THIS_ARGS       [053] NEW                     
[054] RETURN                    [055] RETURN_UNDEF              [056] NEW_OBJECT              
[057] NEW_ARRAY                 [058] GET_PROP                  [059] SET_PROP                
[060] SET_PROP_OBJ              [061] SET_PROP_TOP              [062] GET_ELEM                
[063] SET_ELEM                  [064] SET_ELEM_TOP              [065] DEL_PROP                
[066] DEL_ELEM                  [067] SET_PROP_COMPUTED_OBJ     [068] BUILD_ARRAY             
[069] ARRAY_PUSH                [070] ARRAY_SPREAD              [071] CALL_ARGS               
[072] CALL_METHOD_ARGS          [073] NEW_ARGS                  [074] SPREAD_OBJECT           
[075] UNARY_PLUS                [076] TYPEOF_GLOBAL             [077] TRY_ENTER               
[078] TRY_EXIT                  [079] TRY_EXIT_FINALLY          [080] TRY_EXIT_JMP            
[081] THROW                     [082] INSTANCEOF                [083] IN                      
[084] FOR_IN_NEXT               [085] MAKE_CLASS                [086] GET_PROTO               
[087] CALL_THIS                 [088] CONSTRUCT_THIS            [089] CALL_THIS_ARGS          
[090] CONSTRUCT_THIS_ARGS       [091] GET_ITERATOR              [092] YIELD                   
[093] GET_ASYNC_ITERATOR        [094] AWAIT                     [095] MAKE_REGEXP             
[096] SET_GETTER_OBJ            [097] SET_SETTER_OBJ            [098] SET_GETTER_COMPUTED_OBJ 
[099] SET_SETTER_COMPUTED_OBJ   [100] GET_PROP_LOCAL            [101] CLOSE_UPVALUES          
[102] INC                       [103] DEC                       [104] ENUM_KEYS               
[105] END                     
```
