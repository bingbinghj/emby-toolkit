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
