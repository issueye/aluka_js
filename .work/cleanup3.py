# -*- coding: utf-8 -*-
"""按行号区间删除残留探针块（fmt 后行号已定）。"""
import io

def remove_lines(p, ranges):
    lines = io.open(p, encoding='utf-8').read().split('\n')
    out = []
    skip = set()
    for a, b in ranges:
        for i in range(a, b + 1):
            skip.add(i)
    for i, ln in enumerate(lines, start=1):
        if i in skip:
            continue
        out.append(ln)
    io.open(p, 'w', encoding='utf-8', newline='\n').write('\n'.join(out))
    print("trimmed", p, ranges)

# 行号来自上一轮打印（1-based 闭区间）——注意删除要包含整个块
remove_lines('crates/aluka-vm/src/call.rs', [(508, 530)])          # func8 post-bind 块（含 if 头——需含前导 if 行）
remove_lines('crates/aluka-vm/src/call.rs', [(600, 648)])          # invoke-ret 块
remove_lines('crates/aluka-vm/src/interpreter.rs', [(759, 769)])   # module-scope exports 打印
remove_lines('crates/aluka-vm/src/interpreter.rs', [(1814, 1828)]) # LoadLocal slot2
remove_lines('crates/aluka-vm/src/interpreter.rs', [(1918, 1933)]) # callmethod
remove_lines('crates/aluka-vm/src/interpreter.rs', [(3690, 3742)]) # Call 探针块（含 desc 计算）
remove_lines('crates/aluka-vm/src/property.rs', [(170, 217)])      # get types/extensions 块
print("DONE")
