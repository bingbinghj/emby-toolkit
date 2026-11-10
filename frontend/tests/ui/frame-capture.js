// Test-only: retain actual product RAF callbacks and pixels; never change mode,
// shader uniforms, performance.now(), timers, or canvas contents.
export async function installFrameCaptureGate(page, { paused = false } = {}) {
  await page.addInitScript(({ initiallyPaused }) => {
    const nativeRAF = window.requestAnimationFrame.bind(window);
    const nativeCancel = window.cancelAnimationFrame.bind(window);
    const callbacks = new Map();
    let stopped = initiallyPaused, nextId = 0, executed = 0;
    const schedule = (id, entry) => {
      if (stopped || entry.nativeId !== null || !callbacks.has(id)) return;
      entry.nativeId = nativeRAF(timestamp => {
        entry.nativeId = null;
        if (stopped || !callbacks.delete(id)) return;
        executed += 1;
        entry.callback(timestamp);
      });
    };
    window.requestAnimationFrame = callback => {
      const id = ++nextId;
      const entry = { callback, nativeId: null };
      callbacks.set(id, entry);
      schedule(id, entry);
      return id;
    };
    window.cancelAnimationFrame = id => {
      const entry = callbacks.get(id);
      if (entry?.nativeId !== null && entry?.nativeId !== undefined) nativeCancel(entry.nativeId);
      callbacks.delete(id);
    };
    window.__etkTestFrameCapture = {
      pause() {
        stopped = true;
        for (const entry of callbacks.values()) {
          if (entry.nativeId !== null) nativeCancel(entry.nativeId);
          entry.nativeId = null;
        }
        return { paused: stopped, pending: callbacks.size, executed };
      },
      // Run one real display-frame's current callback batch. Callbacks requested
      // by that batch remain pending until the next step, exactly as with RAF.
      step() {
        if (!stopped) throw new Error('Pause frame capture before stepping');
        return new Promise(resolve => nativeRAF(timestamp => {
          const ids = [...callbacks.keys()];
          const before = executed;
          for (const id of ids) {
            const entry = callbacks.get(id);
            if (!entry) continue;
            callbacks.delete(id);
            executed += 1;
            try { entry.callback(timestamp); }
            catch (error) { setTimeout(() => { throw error; }); }
          }
          resolve({ paused: stopped, pending: callbacks.size, executed, callbacksRun: executed - before, timestamp });
        }));
      },
      resume() {
        stopped = false;
        for (const [id, entry] of callbacks) schedule(id, entry);
        return { paused: stopped, pending: callbacks.size, executed };
      },
      status() { return { paused: stopped, pending: callbacks.size, executed }; },
    };
  }, { initiallyPaused: paused });
}

export const pauseCaptureFrames = page => page.evaluate(() => window.__etkTestFrameCapture.pause());
export const stepCaptureFrame = page => page.evaluate(() => window.__etkTestFrameCapture.step());
export const resumeCaptureFrames = page => page.evaluate(() => window.__etkTestFrameCapture.resume());
