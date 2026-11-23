"""读取酷狗 KRC 的原文、译文和音译，统一为既有 LRC 候选。"""
import base64
import json
import re
import zlib

_KEY = bytes([64, 71, 97, 119, 94, 50, 116, 71, 81, 54, 49, 45, 206, 210, 110, 105])
_MAX_BYTES = 1024 * 1024


def parse_krc(data):
    if not data.startswith(b'krc1'):
        raise ValueError('酷狗 KRC 格式无效')
    compressed = bytes(value ^ _KEY[index % 16] for index, value in enumerate(data[4:]))
    decoder = zlib.decompressobj()
    try:
        raw = decoder.decompress(compressed, _MAX_BYTES + 1)
    except zlib.error as error:
        raise ValueError('酷狗 KRC 内容无效') from error
    if len(raw) > _MAX_BYTES or not decoder.eof or decoder.unused_data:
        raise ValueError('酷狗 KRC 内容不完整或过大')
    text = raw.decode('utf-8-sig')
    rows, headers, language = [], [], ''
    for line in text.splitlines():
        match = re.fullmatch(r'\[(\d{1,10}),(\d{1,10})\](.*)', line)
        if match:
            milliseconds = int(match[1])
            stamp = f'[{milliseconds//60000:02d}:{milliseconds//1000%60:02d}.{milliseconds%1000:03d}]'
            rows.append((stamp, re.sub(r'<\d+,\d+,\d+>', '', match[3])))
        elif re.fullmatch(r'\[(?:ti|ar|al|by|offset):[^\]]*\]', line):
            headers.append(line)
        elif line.startswith('[language:') and line.endswith(']'):
            language = line[10:-1]
    if not any(content.strip() for _, content in rows):
        raise ValueError('酷狗 KRC 没有歌词正文')
    variants = [('original', '原文', '\n'.join(headers + [stamp + content for stamp, content in rows]))]
    try:
        document = json.loads(base64.b64decode(language, validate=True))
    except (ValueError, UnicodeError):
        return variants
    blocks = document.get('content') if isinstance(document, dict) else None
    if not isinstance(blocks, list):
        return variants
    offset = [header for header in headers if header.startswith('[offset:')]
    seen = set()
    for block in blocks:
        if not isinstance(block, dict) or type(block.get('type')) is not int or block['type'] not in (0, 1):
            continue
        kind, title = ('translation', '译文') if block['type'] == 1 else ('romanized', '音译')
        values = block.get('lyricContent')
        # language 包含空行占位；数量不符时不能猜行号，否则会把译文配给别句。
        if kind in seen or not isinstance(values, list) or len(values) != len(rows):
            continue
        if any(not isinstance(chunks, list) or any(not isinstance(chunk, str) or '\n' in chunk or '\r' in chunk for chunk in chunks) for chunks in values):
            continue
        lines = [stamp + ''.join(chunks).strip() for (stamp, _), chunks in zip(rows, values) if ''.join(chunks).strip()]
        if lines:
            variants.append((kind, title, '\n'.join(offset + lines)))
            seen.add(kind)
    return sorted(variants, key=lambda item: ('original', 'translation', 'romanized').index(item[0]))
