/** Include both boundary pages when the visible rows cross a page boundary. */
export function visibleLibraryPageStarts(firstIndex, lastIndex, pageSize) {
  if (lastIndex < firstIndex || pageSize < 1) return [];
  const starts = [];
  for (let start = Math.floor(Math.max(firstIndex, 0) / pageSize) * pageSize;
    start <= lastIndex; start += pageSize) starts.push(start);
  return starts;
}

/** Document scrolling uses window coordinates; nested scroll areas use their own. */
export function libraryViewport(container, documentRoot, windowHeight) {
  return container === documentRoot
    ? { top: 0, height: windowHeight, scrollTop: container.scrollTop }
    : { top: container.getBoundingClientRect().top, height: container.clientHeight, scrollTop: container.scrollTop };
}
