import { onBeforeUnmount, onMounted, shallowRef } from 'vue';

// Keep each page's existing query, filtering and request lifecycle in that page.
export const activeContextSearch = shallowRef(null);

export function useContextSearch(options) {
  const context = { ...options };
  onMounted(() => { activeContextSearch.value = context; });
  onBeforeUnmount(() => {
    // A departing page must not unregister the page that replaced it.
    if (activeContextSearch.value === context) activeContextSearch.value = null;
  });
  return context;
}
