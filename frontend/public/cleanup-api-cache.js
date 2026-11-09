// Remove the old API cache when this worker takes control; keep static PWA caches.
self.addEventListener('activate', event => {
  event.waitUntil(caches.delete('api-cache'));
});
