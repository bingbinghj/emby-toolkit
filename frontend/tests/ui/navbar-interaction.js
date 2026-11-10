// Keep real pointer input and captured pixels away from native controls.
export async function navbarInteractionRegion(page) {
  return page.locator('.layout-navbar').evaluate(navbar => {
    const visibleBox = element => {
      if (!element || !element.getClientRects().length || getComputedStyle(element).visibility === 'hidden') return null;
      const { x, y, width, height } = element.getBoundingClientRect();
      return width && height ? { x, y, width, height } : null;
    };
    const navbarBox = visibleBox(navbar);
    const search = visibleBox(navbar.querySelector('.navbar-search-desktop'));
    const actions = visibleBox(navbar.querySelector('.theme-navbar-actions'));
    if (!navbarBox) throw new Error('Navbar must be visible before interaction capture');
    let left = navbarBox.x + Math.min(160, navbarBox.width / 4);
    let width = Math.min(400, navbarBox.width / 2);
    if (search) {
      if (!actions) throw new Error('Desktop navbar actions must be visible');
      left = search.x + search.width + 8;
      const gap = actions.x - 8 - left;
      if (gap < 80) throw new Error(`Desktop navbar has no usable blank gap: ${gap}px`);
      width = Math.min(400, gap);
      left += (gap - width) / 2;
    }
    const clip = { x: Math.ceil(left), y: Math.ceil(navbarBox.y + 2),
      width: Math.floor(width), height: Math.floor(Math.min(50, navbarBox.height - 4)) };
    const interactive = 'input, button, a, select, textarea, [role="button"], [contenteditable="true"]';
    for (const element of navbar.querySelectorAll(interactive)) {
      const box = visibleBox(element);
      if (box && box.x < clip.x + clip.width && box.x + box.width > clip.x
        && box.y < clip.y + clip.height && box.y + box.height > clip.y) {
        throw new Error(`Interaction capture overlaps ${element.tagName}: ${JSON.stringify({ clip, box })}`);
      }
    }
    const start = { x: clip.x + clip.width * .4, y: clip.y + clip.height * .5 };
    const end = { x: clip.x + clip.width * .6, y: clip.y + clip.height * .55 };
    const pointerTargets = [];
    for (let step = 0; step <= 6; step += 1) {
      const target = document.elementFromPoint(start.x + (end.x - start.x) * step / 6,
        start.y + (end.y - start.y) * step / 6);
      if (!target || !navbar.contains(target) || target.closest(interactive)) {
        throw new Error(`Interaction pointer must hit blank navbar: ${target?.tagName ?? 'none'}`);
      }
      pointerTargets.push(target.className);
    }
    return { clip, start, end, navbar: navbarBox, search, actions, pointerTargets };
  });
}
