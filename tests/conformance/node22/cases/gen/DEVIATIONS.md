# 生成语料偏差清单（partition.py 自动产出；修复后重跑即自动回归 gen/）

> 语料: 838 通过 / 345 偏差 / 0 无效对比（本轮扫描 1008）

## gen-array-0037.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 3
  vm  : undefined
```

## gen-array-0039.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 1
  vm  : undefined
```

## gen-array-0040.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 3
  vm  : undefined
```

## gen-builtin-buffer-0005.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 0
  vm  : ERR TypeError
```

## gen-builtin-buffer-0006.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "Xello"
  vm  : ERR TypeError
```

## gen-builtin-buffer-0008.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 2
  vm  : ERR TypeError
```

## gen-builtin-buffer-0012.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 3
  vm  : undefined
```

## gen-builtin-buffer-0015.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 97
  vm  : ERR TypeError
```

## gen-builtin-fs-0003.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: false
  vm  : ERR TypeError
```

## gen-builtin-fs-0004.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "f.txt"
  vm  : ERR undefined
```

## gen-builtin-fs-0007.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "ENOENT"
  vm  : undefined
```

## gen-builtin-fs-0008.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "5"
  vm  : ERR TypeError
```

## gen-builtin-fs-0010.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: ERR Error
  vm  : ERR undefined
```

## gen-builtin-fs-0011.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "ab"
  vm  : ERR TypeError
```

## gen-builtin-os-0005.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "string"
  vm  : ERR TypeError
```

## gen-builtin-path-0002.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "\\b"
  vm  : "\\a\\..\\b"
```

## gen-builtin-path-0006.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "/a/b"
  vm  : "\\a\\b"
```

## gen-builtin-path-0009.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: true
  vm  : ERR TypeError
```

## gen-builtin-path-0010.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: ".txt"
  vm  : ERR TypeError
```

## gen-builtin-path-0011.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "a\\b\\d"
  vm  : ERR TypeError
```

## gen-builtin-path-0013.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: true
  vm  : ERR TypeError
```

## gen-builtin-stream-0001.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "ab"
  vm  : ERR undefined
```

## gen-builtin-stream-0004.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "PQ"
  vm  : ""
```

## gen-builtin-stream-0006.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "z"
  vm  : ERR TypeError
```

## gen-builtin-timers-0006.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "2,object,function"
  vm  : "1,object,function"
```

## gen-builtin-timers-0010.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "true,true"
  vm  : "false,undefined"
```

## gen-builtin-url-0001.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: {"a":"1","b":"x y"}
  vm  : {"a":"1","b":"x%20y"}
```

## gen-builtin-url-0002.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "a=1&b=x%20y"
  vm  : "a=1&b=x+y"
```

## gen-builtin-url-0004.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "2"
  vm  : undefined
```

## gen-builtin-url-0005.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "?k=v+v"
  vm  : ""
```

## gen-builtin-url-0006.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "https://x.io/rel"
  vm  : "/rel"
```

## gen-builtin-url-0008.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "user"
  vm  : undefined
```

## gen-builtin-util-0002.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "{\"k\":1}"
  vm  : "{ k: 1 }"
```

## gen-builtin-util-0003.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: true
  vm  : ERR TypeError
```

## gen-builtin-util-0005.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: true
  vm  : ERR TypeError
```

## gen-builtin-util-0007.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "AssertionError"
  vm  : undefined
```

## gen-builtin-util-0008.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "deep-ok"
  vm  : ERR TypeError
```

## gen-builtin-util-0009.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "AssertionError"
  vm  : undefined
```

## gen-class-proto-0001.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "am"
  vm  : ERR TypeError
```

## gen-class-proto-0002.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: true
  vm  : false
```

## gen-class-proto-0003.cjs
```
alukac 编译失败: 错误: 解析源文件 "E:\codes\go_projects\aluka_lang\aluka_wt_npm_m6m7\tests/conformance/node22/cases\gen\gen-class-proto-0003.cjs" 失败: SyntaxError: 预期标点 '(', 实为 'Token { kind: Ident("v"), text: "v", start: 130 }'; 预期标点 ')', 实为 'Token { kind: Punct("("), text: "(", start: 131 }'; 预期标点 ')', 实为 'Token { kind: Punct("{"), text: "{", start: 134 }'; 预期标点 '}', 实为 'Token { kind: Punct("{"), text: "{", start: 134 }'; 预期标点 ')', 实为 'Token { kind: Keyword("return"), text: "return", start: 149 }'; 预期标点 ')', 实为 'Token { kind: Keyword("return"), text: "return", start: 149 }'; 预期标点 ')', 实为 'Token { kind: Keyword("return"), text: "return", start: 149 }' (E:\codes\go_projects\aluka_lang\aluka_wt_npm_m6m7\tests/conformance/node22/cases\gen\gen-class-proto-0003.cjs)
```

## gen-class-proto-0004.cjs
```
alukac 编译失败: 错误: 解析源文件 "E:\codes\go_projects\aluka_lang\aluka_wt_npm_m6m7\tests/conformance/node22/cases\gen\gen-class-proto-0004.cjs" 失败: SyntaxError: 预期标点 '(', 实为 'Token { kind: Ident("s"), text: "s", start: 133 }'; 预期标点 ')', 实为 'Token { kind: Punct("("), text: "(", start: 134 }'; 预期标点 ')', 实为 'Token { kind: Punct("{"), text: "{", start: 137 }'; 预期标点 '}', 实为 'Token { kind: Punct("{"), text: "{", start: 137 }'; 预期标点 ')', 实为 'Token { kind: Keyword("return"), text: "return", start: 155 }'; 预期标点 ')', 实为 'Token { kind: Keyword("return"), text: "return", start: 155 }'; 预期标点 ')', 实为 'Token { kind: Keyword("return"), text: "return", start: 155 }' (E:\codes\go_projects\aluka_lang\aluka_wt_npm_m6m7\tests/conformance/node22/cases\gen\gen-class-proto-0004.cjs)
```

## gen-class-proto-0005.cjs
```
alukac 编译失败: 错误: 解析源文件 "E:\codes\go_projects\aluka_lang\aluka_wt_npm_m6m7\tests/conformance/node22/cases\gen\gen-class-proto-0005.cjs" 失败: SyntaxError: 预期标点 '}', 实为 'Token { kind: Punct("#"), text: "#", start: 126 }'; 预期标点 ')', 实为 'Token { kind: Keyword("return"), text: "return", start: 161 }'; 预期标点 ')', 实为 'Token { kind: Keyword("return"), text: "return", start: 161 }'; 预期标点 ')', 实为 'Token { kind: Keyword("return"), text: "return", start: 161 }' (E:\codes\go_projects\aluka_lang\aluka_wt_npm_m6m7\tests/conformance/node22/cases\gen\gen-class-proto-0005.cjs)
```

## gen-class-proto-0006.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: [1,2]
  vm  : [null,null]
```

## gen-class-proto-0009.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "function"
  vm  : ERR TypeError
```

## gen-class-proto-0010.cjs
```
alukac 编译失败: 错误: 解析源文件 "E:\codes\go_projects\aluka_lang\aluka_wt_npm_m6m7\tests/conformance/node22/cases\gen\gen-class-proto-0010.cjs" 失败: SyntaxError: 预期标点 '(', 实为 'Token { kind: Punct("{"), text: "{", start: 133 }'; 预期标点 ')', 实为 'Token { kind: Ident("A"), text: "A", start: 135 }'; 预期标点 ')', 实为 'Token { kind: Keyword("return"), text: "return", start: 151 }'; 预期标点 ')', 实为 'Token { kind: Keyword("return"), text: "return", start: 151 }'; 预期标点 ')', 实为 'Token { kind: Keyword("return"), text: "return", start: 151 }' (E:\codes\go_projects\aluka_lang\aluka_wt_npm_m6m7\tests/conformance/node22/cases\gen\gen-class-proto-0010.cjs)
```

## gen-coerce-matrix-0012.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 2
  vm  : null
```

## gen-coerce-matrix-0019.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: ",,"
  vm  : "undefined,undefined,undefined"
```

## gen-core-pairs-0008.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "Number"
  vm  : ERR TypeError
```

## gen-core-pairs-0009.cjs
```
node: undefined
vm  : <compile fail> 错误: 解析源文件 "C:\Users\issue\AppData\Local\Temp\tmpa6z2gns4\gen-core-pairs-0009.cjs" 失败: SyntaxError: 预期标点 ')', 实为 'Token { kind: Ident("constructor"), text: "constructor", start: 109 }'; 预期标点 ')', 实为 'T
```

## gen-core-pairs-0012.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: true
  vm  : false
```

## gen-core-pairs-0013.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: true
  vm  : false
```

## gen-core-pairs-0014.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: true
  vm  : false
```

## gen-core-pairs-0015.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: true
  vm  : false
```

## gen-core-pairs-0016.cjs
```
node: true
vm  : false
```

## gen-core-pairs-0018.cjs
```
alukac 编译失败: 错误: 解析源文件 "E:\codes\go_projects\aluka_lang\aluka_wt_npm_m6m7\tests/conformance/node22/cases\gen\gen-core-pairs-0018.cjs" 失败: SyntaxError: 预期标点 ')', 实为 'Token { kind: Punct(","), text: ",", start: 108 }' (E:\codes\go_projects\aluka_lang\aluka_wt_npm_m6m7\tests/conformance/node22/cases\gen\gen-core-pairs-0018.cjs)
```

## gen-core-pairs-0019.cjs
```
node: 7
vm  : <compile fail> 错误: 解析源文件 "C:\Users\issue\AppData\Local\Temp\tmpidr_sopd\gen-core-pairs-0019.cjs" 失败: SyntaxError: 预期标点 ')', 实为 'Token { kind: Punct(","), text: ",", start: 108 }' (C:\Users\issue\AppData\Local\Temp\t
```

## gen-core-pairs-0025.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 10
  vm  : undefined
```

## gen-core-pairs-0026.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 10
  vm  : undefined
```

## gen-core-pairs-0027.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 2
  vm  : undefined
```

## gen-core-pairs-0028.cjs
```
node: 2
vm  : undefined
```

## gen-date-matrix-0001.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 0
  vm  : undefined
```

## gen-date-matrix-0002.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "1970-01-01T00:00:00.000Z"
  vm  : undefined
```

## gen-date-matrix-0003.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "1970-01-02T00:00:00.000Z"
  vm  : undefined
```

## gen-date-matrix-0004.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "1970-01-01T00:00:00.000Z"
  vm  : ERR TypeError
```

## gen-date-matrix-0006.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 0
  vm  : ERR TypeError
```

## gen-date-matrix-0007.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: true
  vm  : false
```

## gen-date-matrix-0010.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: null
  vm  : undefined
```

## gen-error-0004.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "RangeError"
  vm  : undefined
```

## gen-error-0005.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "URIError"
  vm  : undefined
```

## gen-error-0009.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: false
  vm  : true
```

## gen-eval-matrix-0008.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: true
  vm  : false
```

## gen-final-matrix-0001.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "{\"x\":1}"
  vm  : "{}"
```

## gen-final-matrix-0005.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 0
  vm  : undefined
```

## gen-final-matrix-0008.cjs
```
alukac 编译失败: 错误: 解析源文件 "E:\codes\go_projects\aluka_lang\aluka_wt_npm_m6m7\tests/conformance/node22/cases\gen\gen-final-matrix-0008.cjs" 失败: SyntaxError: 预期标点 ')', 实为 'Token { kind: Ident("toFixed"), text: "toFixed", start: 112 }'; 预期标点 ')', 实为 'Token { kind: Ident("toFixed"), text: "toFixed", start: 112 }'; 预期标点 ')', 实为 'Token { kind: Ident("toFixed"), text: "toFixed", start: 112 }' (E:\codes\go_projects\aluka_lang\aluka_wt_npm_m6m7\tests/conformance/node22/cases\gen\gen-final-matrix-0008.cjs)
```

## gen-final-matrix-0010.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 9
  vm  : ERR TypeError
```

## gen-final-matrix-0013.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: false
  vm  : true
```

## gen-final-matrix-0019.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 5
  vm  : 0
```

## gen-final-matrix-0020.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 5
  vm  : 0
```

## gen-final-matrix-0021.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 1
  vm  : 0
```

## gen-global-matrix-0018.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "object"
  vm  : "undefined"
```

## gen-json-matrix-0012.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: {"a":2}
  vm  : {"a":1}
```

## gen-lang-core-0060.cjs
```
alukac 编译失败: 错误: 解析源文件 "E:\codes\go_projects\aluka_lang\aluka_wt_npm_m6m7\tests/conformance/node22/cases\gen\gen-lang-core-0060.cjs" 失败: SyntaxError: 预期标点 ')', 实为 'Token { kind: Punct("{"), text: "{", start: 118 }'; 预期标点 ')', 实为 'Token { kind: Punct("{"), text: "{", start: 118 }' (E:\codes\go_projects\aluka_lang\aluka_wt_npm_m6m7\tests/conformance/node22/cases\gen\gen-lang-core-0060.cjs)
```

## gen-lang-core-0066.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 2
  vm  : null
```

## gen-lang-core-0067.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 1
  vm  : null
```

## gen-lang-core-0070.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: true
  vm  : false
```

## gen-lang-core-0083.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: ERR ReferenceError
  vm  : undefined
```

## gen-lang-more-0001.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: [1,2]
  vm  : []
```

## gen-lang-more-0003.cjs
```
alukac 编译失败: 错误: 解析源文件 "E:\codes\go_projects\aluka_lang\aluka_wt_npm_m6m7\tests/conformance/node22/cases\gen\gen-lang-more-0003.cjs" 失败: SyntaxError: 预期标点 ']', 实为 'Token { kind: Ident("z"), text: "z", start: 126 }'; 预期标点 '=', 实为 'Token { kind: Ident("z"), text: "z", start: 126 }' (E:\codes\go_projects\aluka_lang\aluka_wt_npm_m6m7\tests/conformance/node22/cases\gen\gen-lang-more-0003.cjs)
```

## gen-lang-more-0024.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: [2,1,0]
  vm  : []
```

## gen-map-matrix-0005.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 2
  vm  : 0
```

## gen-map-matrix-0006.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 2
  vm  : 0
```

## gen-map-set-0001.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: [[1,"a"],[2,"b"]]
  vm  : []
```

## gen-map-set-0009.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: ["a","b","c"]
  vm  : []
```

## gen-map-set-0015.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 2
  vm  : ERR TypeError
```

## gen-math-matrix-0053.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 0.7937005259840998
  vm  : 0.7937005259840997
```

## gen-math-matrix-0054.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: -1.3572088082974532
  vm  : -1.3572088082974534
```

## gen-math-matrix-0056.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 0.1
  vm  : 0.09999999999999999
```

## gen-math-matrix-0064.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 0
  vm  : ERR TypeError
```

## gen-math-matrix-0065.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 0.6931471805599453
  vm  : ERR TypeError
```

## gen-math-matrix-0066.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: null
  vm  : ERR TypeError
```

## gen-math-matrix-0067.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 0.4054651081081644
  vm  : ERR TypeError
```

## gen-math-matrix-0068.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: null
  vm  : ERR TypeError
```

## gen-math-matrix-0069.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 4.61512051684126
  vm  : ERR TypeError
```

## gen-math-matrix-0070.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 0.0009995003330835331
  vm  : ERR TypeError
```

## gen-math-number-0016.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 0
  vm  : ERR TypeError
```

## gen-math-number-0017.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 1
  vm  : ERR TypeError
```

## gen-math-number-0018.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 0.7853981633974483
  vm  : ERR TypeError
```

## gen-math-number-0034.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "1.5"
  vm  : "1.5e0"
```

## gen-math-number-0035.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "1.23e+3"
  vm  : "1.23e3"
```

## gen-math-number-0037.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: false
  vm  : true
```

## gen-misc-console-0002.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "erred" | err-line
  vm  : "erred"
```

## gen-misc-console-0003.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: info-line | "infoed"
  vm  : "infoed"
```

## gen-misc-console-0004.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "warned" | warn-line
  vm  : "warned"
```

## gen-misc-console-0006.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: { a: [ 1, 2 ] } | "obj"
  vm  : [object Object] | "obj"
```

## gen-num-matrix-0004.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "7b.74bc6a7ef9dc"
  vm  : "7b"
```

## gen-num-matrix-0005.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "123"
  vm  : "123.46"
```

## gen-num-matrix-0016.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "-2a.b33333333334"
  vm  : "-2a"
```

## gen-num-matrix-0017.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "-42.7"
  vm  : "-4.27e1"
```

## gen-num-matrix-0023.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "255"
  vm  : "255.00"
```

## gen-num-matrix-0025.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "1"
  vm  : "0"
```

## gen-num-matrix-0028.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "0.8"
  vm  : "0"
```

## gen-num-matrix-0029.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "0.500"
  vm  : "5.00e-1"
```

## gen-obj-matrix-0017.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: ["1970-01-01T00:00:00.000Z"]
  vm  : [{"_builtinNs":"Date","_isDate":true,"_timeValue":0}]
```

## gen-obj-matrix-0018.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "{\"d\":\"1970-01-01T00:00:00.000Z\"}"
  vm  : "{\"d\":{\"_builtinNs\":\"Date\",\"_isDate\":true,\"_timeValue\":0}}"
```

## gen-obj-matrix-0019.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: {"d":"1970-01-01T00:00:00.000Z"}
  vm  : {"d":{"_builtinNs":"Date","_isDate":true,"_timeValue":0}}
```

## gen-object-json-0006.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: ["0","1","length"]
  vm  : []
```

## gen-object-json-0010.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "\"j\""
  vm  : "{}"
```

## gen-object-json-0015.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: ERR ReferenceError
  vm  : {"a":1}
```

## gen-object-json-0024.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: {"a":[1,{"b":"c"}]}
  vm  : ERR TypeError
```

## gen-object-json-0025.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "object"
  vm  : ERR TypeError
```

## gen-path-matrix-0006.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "/a/b"
  vm  : "\\a\\b"
```

## gen-path-matrix-0008.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "."
  vm  : ""
```

## gen-path-matrix-0009.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "/"
  vm  : ""
```

## gen-path-matrix-0010.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "./x"
  vm  : ".\\x"
```

## gen-promise-async-0009.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: {}
  vm  : ERR TypeError
```

## gen-promise-matrix-0001.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "function"
  vm  : "undefined"
```

## gen-promise-matrix-0002.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "function"
  vm  : "undefined"
```

## gen-promise-matrix-0003.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "function"
  vm  : "undefined"
```

## gen-promise-matrix-0004.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "function"
  vm  : "undefined"
```

## gen-promise-matrix-0005.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "function"
  vm  : "undefined"
```

## gen-promise-matrix-0006.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "function"
  vm  : "undefined"
```

## gen-promise-matrix-0007.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "function"
  vm  : "undefined"
```

## gen-promise-matrix-0008.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "function"
  vm  : "undefined"
```

## gen-promise-matrix-0009.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "function"
  vm  : "undefined"
```

## gen-promise-matrix-0010.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: true
  vm  : false
```

## gen-regex-matrix-0008.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 0
  vm  : ERR undefined
```

## gen-regex-matrix-0009.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: undefined
  vm  : ERR TypeError
```

## gen-regexp-0004.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: ERR ReferenceError
  vm  : null
```

## gen-set-matrix-0004.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 2
  vm  : 0
```

## gen-set-matrix-0006.cjs
```
node: 2
vm  : 0
```

## gen-set-matrix-0007.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: [1,2,3]
  vm  : []
```

## gen-set-matrix-0008.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: [1,2,3]
  vm  : []
```

## gen-set-matrix-0009.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: ["a","b"]
  vm  : []
```

## gen-set-matrix-0010.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: ["a","b"]
  vm  : []
```

## gen-str-matrix-0021.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "....Hello World"
  vm  : ERR undefined
```

## gen-str-matrix-0022.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "Hello World----"
  vm  : ERR undefined
```

## gen-str-matrix-0043.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "..............."
  vm  : ERR undefined
```

## gen-str-matrix-0044.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "---------------"
  vm  : ERR undefined
```

## gen-str-matrix-0065.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "..............a"
  vm  : ERR undefined
```

## gen-str-matrix-0066.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "a--------------"
  vm  : ERR undefined
```

## gen-str-matrix-0087.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: ".........abc123"
  vm  : ERR undefined
```

## gen-str-matrix-0088.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "abc123---------"
  vm  : ERR undefined
```

## gen-str-matrix-0109.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: ".  mixEd Case  "
  vm  : ERR undefined
```

## gen-str-matrix-0110.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "  mixEd Case  -"
  vm  : ERR undefined
```

## gen-string-0004.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "c"
  vm  : ERR undefined
```

## gen-string-0016.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "**abc"
  vm  : ERR undefined
```

## gen-string-0017.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "05"
  vm  : ERR undefined
```

## gen-string-0018.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "abc  "
  vm  : ERR undefined
```

## gen-string-0022.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 97
  vm  : ERR undefined
```

## gen-string-0025.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 2
  vm  : ERR undefined
```

## gen-string-0031.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: "a\nb"
  vm  : ERR TypeError
```

## gen-string-0036.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 2
  vm  : 1
```

## gen-string-0037.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: 128512
  vm  : ERR undefined
```

## gen-symbol-matrix-0007.cjs
```
stdout 不一致 (node_rc=Some(0) vm_rc=Some(0))
  node: [1]
  vm  : []
```
