import test from 'node:test';
import assert from 'node:assert/strict';
import { libraryViewport, visibleLibraryPageStarts } from '../src/utils/mediaLibraryNavigation.js';

test('visible rows crossing a page boundary load the last boundary page', () => {
  assert.deepEqual(visibleLibraryPageStarts(17, 43, 30), [0, 30]);
  assert.deepEqual(visibleLibraryPageStarts(92, 145, 30), [90, 120]);
  assert.deepEqual(visibleLibraryPageStarts(0, -1, 30), []);
});

test('document viewport is measured independently from the entire document height', () => {
  const root = { scrollTop: 1400, clientHeight: 8000 };
  assert.deepEqual(libraryViewport(root, root, 900), { top: 0, height: 900, scrollTop: 1400 });
  const panel = { scrollTop: 320, clientHeight: 600, getBoundingClientRect: () => ({ top: 100 }) };
  assert.deepEqual(libraryViewport(panel, root, 900), { top: 100, height: 600, scrollTop: 320 });
});
