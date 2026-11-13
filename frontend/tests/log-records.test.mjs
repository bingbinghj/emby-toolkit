import test from 'node:test';
import assert from 'node:assert/strict';
import { parseLogRecord, matchesLogRecord, highlightLogText, groupLogRecords } from '../src/utils/logRecords.js';

const full = '2026-11-10 12:30:45,123 - emby_toolkit::media_reload - CRITICAL - 媒体 JSON 在失败后被修改，保留当前文件及备份\n原始路径 /config/backups/结尾';

test('history record parsing preserves complete multiline body, raw export and critical level', () => {
  const record = parseLogRecord(full);
  assert.equal(record.timestamp, '2026-11-10 12:30:45,123');
  assert.equal(record.level, 'CRITICAL');
  assert.equal(record.module, 'emby_toolkit::media_reload');
  assert.equal(record.message, '媒体 JSON 在失败后被修改，保留当前文件及备份\n原始路径 /config/backups/结尾');
  assert.equal(record.raw, full);
});

test('live records and unformatted continuations keep all body text', () => {
  assert.deepEqual(parseLogRecord('[12:30:45] [WARN] 完整文本\n结束'), { raw: '[12:30:45] [WARN] 完整文本\n结束', timestamp: '12:30:45', level: 'WARNING', module: '', message: '完整文本\n结束' });
  assert.equal(parseLogRecord('  continuation <script> & 结束').message, '  continuation <script> & 结束');
});

test('Rust targets and Python module function line locators retain every source character', () => {
  for (const source of ['emby_toolkit::media_reload', 'app.modules.emby.media_info:restore_saved_json:354']) {
    const raw = `2026-11-10 12:30:45,123 - ${source} - WARNING - 完整来源与结尾`;
    const record = parseLogRecord(raw);
    assert.equal(record.module, source);
    assert.equal(record.raw, raw);
    assert.equal(record.message, '完整来源与结尾');
    assert.equal(matchesLogRecord(raw, source, 'warning'), true);
  }
});

test('keyword and level filtering supports both actual log formats and error includes critical', () => {
  assert.equal(matchesLogRecord(full, 'json', 'error'), true);
  assert.equal(matchesLogRecord(full, 'json', 'warning'), false);
  assert.equal(matchesLogRecord('[01:02:03] [INFO] [任务进度] 进度详情', '详情', 'progress'), true);
  assert.equal(matchesLogRecord('[01:02:03] [WARNING] [任务进度] 详情', '', 'warning'), true);
  assert.equal(matchesLogRecord('正文', '不存在', 'all'), false);
});

test('safe keyword highlighting handles literal regex characters and never turns markup into HTML', () => {
  const text = '<script>[JSON].*(a)</script> [json].*(a)';
  const parts = highlightLogText(text, '[JSON].*(a)');
  assert.equal(parts.map(part => part.text).join(''), text);
  assert.equal(parts.filter(part => part.matched).length, 2);
  assert.equal(parts[0].text, '<script>');
  assert.deepEqual(highlightLogText('abc', ''), [{ text: 'abc', matched: false }]);
});

test('MP log groups merge only adjacent records of the same second and level within 100 ms', () => {
  const line = (time, level, body) => `2026-11-11 12:30:45,${time} - source - ${level} - ${body}`;
  const lines = [line('001', 'INFO', '第一条'), line('050', 'INFO', '第二条'), line('250', 'INFO', '间隔较大'), line('260', 'WARNING', '级别不同'), '续行原文'];
  const groups = groupLogRecords(lines);
  assert.deepEqual(groups.map(group => group.lines.length), [2, 1, 1, 1]);
  assert.deepEqual(groups.flatMap(group => group.lines), lines);
  assert.equal(groups[0].second, '2026-11-11 12:30:45');
  assert.equal(groups[0].level, 'INFO');
});

test('native live dates remain distinct across days and retain precise grouping times', () => {
  const lines = [
    '2026-11-11 09:56:24,990 - source - INFO - 昨日记录',
    '2026-11-12 09:56:24,010 - source - INFO - 今日记录',
    '2026-11-12 09:56:24,090 - source - INFO - 同秒记录',
  ];
  const groups = groupLogRecords(lines);
  assert.deepEqual(groups.map(group => group.lines.length), [1, 2]);
  assert.equal(parseLogRecord(lines[1]).timestamp, '2026-11-12 09:56:24,010');
  assert.deepEqual(groups.flatMap(group => group.lines), lines);
});

test('out-of-order timestamps cannot merge records separated by more than 100 ms', () => {
  const line = (time, body) => `2026-11-12 12:30:45,${time} - source - INFO - ${body}`;
  const lines = [line('900', 'later'), line('100', 'earlier'), line('050', 'close earlier'), line('300', 'later again')];
  const groups = groupLogRecords(lines);
  assert.deepEqual(groups.map(group => group.lines.length), [1, 2, 1]);
  assert.deepEqual(groups.flatMap(group => group.lines), lines);
});

test('legacy time-only live records keep their unknown-precision grouping and original order', () => {
  const lines = ['[12:30:45] [INFO] first', '[12:30:45] [INFO] second', '[12:30:45] [ERROR] third'];
  const groups = groupLogRecords(lines);
  assert.deepEqual(groups.map(group => group.lines.length), [2, 1]);
  assert.deepEqual(groups.flatMap(group => group.lines), lines);
});

test('MP level-first logs preserve the source, milliseconds and multiline message', () => {
  for (const timestamp of ['2026-11-13 13:19:14,476', '13:19:14,476']) {
    const raw = `INFO:${timestamp} sync.rs - [同步媒体] 媒体库 21 扫描完成\n下一行顶格显示`;
    assert.deepEqual(parseLogRecord(raw), { raw, timestamp, level: 'INFO', module: 'sync.rs', message: '[同步媒体] 媒体库 21 扫描完成\n下一行顶格显示' });
    assert.equal(matchesLogRecord(raw, 'sync.rs', 'info'), true);
  }
});

test('MP level-first dates retain their day and exact grouping gap', () => {
  const lines = [
    'INFO: 2026-11-12 23:59:59,990 sync.rs - 昨日记录',
    'INFO: 2026-11-13 00:00:00,010 sync.rs - 今日记录',
    'INFO: 2026-11-13 00:00:00,090 sync.rs - 同秒记录',
    'INFO: 2026-11-13 00:00:00,300 sync.rs - 不同组',
  ];
  assert.deepEqual(groupLogRecords(lines).map(group => group.lines.length), [1, 2, 1]);
  assert.deepEqual(groupLogRecords(lines).flatMap(group => group.lines), lines);
});
