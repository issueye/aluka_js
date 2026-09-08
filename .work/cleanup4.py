# -*- coding: utf-8 -*-
"""结构化删除调试探针块：从含特征锚点的行起，按大括号配对删除整个 if 块。"""
import io

def strip_blocks(p, anchors):
    lines = io.open(p, encoding='utf-8').read().split('\n')
    out = []
    i = 0
    removed = 0
    while i < len(lines):
        # 块起点：行内含锚点（该行以 if std::env::var 或 let 开头或内含）
        start = None
        for a in anchors:
            if a in lines[i]:
                start = i
                break
        if start is None:
            out.append(lines[i])
            i += 1
            continue
        # 从 start 起配对括号：统计从该行 '{' 开始
        depth = 0
        j = start
        opened = False
        while j < len(lines):
            depth += lines[j].count('{') - lines[j].count('}')
            if '{' in lines[j]:
                opened = True
            if opened and depth <= 0:
                break
            j += 1
        # 删除 start..j（若 j 越界则删到末尾——保守只删到块闭合）
        if j < len(lines):
            removed += 1
        i = j + 1
    io.open(p, 'w', encoding='utf-8', newline='\n').write('\n'.join(out))
    print("stripped", p, "blocks:", removed)

# call.rs：func8 post-bind 与 invoke-ret 两块（锚点是块的 if 首行）
strip_blocks('crates/aluka-vm/src/call.rs', [
    'if std::env::var("ALUKA_REQ_DEBUG").is_ok() && func_idx == 8 {',
    'if std::env::var("ALUKA_REQ_DEBUG").is_ok()',
])
print("note: call.rs 通用 if 锚点会误删保留块——需单独核对")
