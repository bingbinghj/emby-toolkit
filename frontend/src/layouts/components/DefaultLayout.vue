<!-- Adapted from MoviePilot-Frontend 609ea05, default layout and footer. Copyright (c) 2023 jxxghp, MIT; see ../LICENSE.MoviePilot. -->
<script setup>
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useRoute } from 'vue-router';
import { useDisplay, useTheme } from 'vuetify';
import { useEffectiveGlassSettings } from '@/composables/useThemeCustomizer';
import { usePWA } from '@/utils/browserCapabilities';
import {
  isChromiumFixedShellBackplateBrowser,
  shouldUseGlassFixedShellBackplate,
  useGlassFixedShellBackplate,
} from '@/composables/useGlassFixedShellBackplate';
import GlassFixedShellBackplate from '@/components/theme/GlassFixedShellBackplate.vue';
import GlassNavbarRefractionDefs from '@/components/theme/GlassNavbarRefractionDefs.vue';
import GlassPanelRefractionDefs from '@/components/theme/GlassPanelRefractionDefs.vue';

const props = defineProps({
  menus: { type: Array, default: () => [] },
  activeKey: String,
  layout: { type: String, default: 'vertical' },
  expandedKeys: { type: Array, default: () => [] },
  menuRevision: { type: Number, default: 0 },
  hasHeaderTabs: Boolean,
});
const emit = defineEmits(['navigate', 'update:layout']);
const route = useRoute();
const { mdAndDown, width } = useDisplay();
const overlayOpen = ref(false), openHorizontalGroup = ref(null), opened = ref([]);
const collapsed = computed(() => !mdAndDown.value && props.layout === 'collapsed');
const horizontal = computed(() => !mdAndDown.value && props.layout === 'horizontal');
const theme = useTheme();
const glass = useEffectiveGlassSettings();
const fixedShell = useGlassFixedShellBackplate();
const { displayEnvironment, isWindowControlsOverlayMode } = usePWA();
const isChromium = isChromiumFixedShellBackplateBrowser();
const supportsRefraction = computed(() => isChromium && theme.global.name.value === 'glass');
const glassShellKey = computed(() => `${horizontal.value ? 'horizontal' : 'vertical'}-${mdAndDown.value ? 'app' : 'desktop'}`);
const showFixedBackplate = computed(() => !horizontal.value && !mdAndDown.value && !isWindowControlsOverlayMode.value
  && shouldUseGlassFixedShellBackplate({
    appearance: glass.value.glassAppearance,
    hasWallpaper: fixedShell.layers.value.length > 0,
    isAuthenticated: !route.meta.public,
    needsStableFixedBackdrop: isChromium,
    quality: glass.value.glassQuality,
    themeName: theme.global.name.value,
  }));
const flattenedMenus = computed(() => props.menus.flatMap(menu => menu.children || [menu]));
const dockMenus = computed(() => flattenedMenus.value.slice(0, 3));
const scrollProgress = ref(0);
const navbarAwayFromTop = ref(false), floatingNavbarScale = ref(1);
let scrollFrame;

watch([() => props.expandedKeys, () => props.menuRevision], ([keys]) => { opened.value = [...keys]; }, { immediate: true, deep: true });
watch(() => route.fullPath, () => {
  overlayOpen.value = false;
  openHorizontalGroup.value = null;
});
watch(mdAndDown, value => { overlayOpen.value = false; if (value) navbarAwayFromTop.value = false; else updateScroll(); });
function updateFloatingNavbarGeometry() {
  const viewportWidth = document.documentElement.clientWidth;
  const inset = Number.parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
  floatingNavbarScale.value = viewportWidth > inset * 2 ? (viewportWidth - inset * 2) / viewportWidth : 1;
}
watch([horizontal, width], updateFloatingNavbarGeometry, { flush: 'post' });

function navigate(key) {
  overlayOpen.value = false;
  openHorizontalGroup.value = null;
  emit('navigate', key);
}
function toggleNavigation() {
  overlayOpen.value = !overlayOpen.value;
}
function groupActive(group) {
  return (group.children || [group]).some(item => item.key === props.activeKey);
}
function updateScroll() {
  if (scrollFrame) return;
  scrollFrame = requestAnimationFrame(() => {
    scrollFrame = undefined;
    const scrollY = window.scrollY;
    const progress = Math.min(1, Math.max(0, scrollY / 200));
    scrollProgress.value = progress * progress * (3 - 2 * progress);
    if (mdAndDown.value || scrollY <= 4) navbarAwayFromTop.value = false;
    else if (scrollY >= 12) navbarAwayFromTop.value = true;
  });
}
onMounted(() => { window.addEventListener('scroll', updateScroll, { passive: true }); updateFloatingNavbarGeometry(); updateScroll(); });
onBeforeUnmount(() => { window.removeEventListener('scroll', updateScroll); cancelAnimationFrame(scrollFrame); });
</script>

<template>
  <VLayout class="layout-wrapper layout-nav-type-vertical layout-navbar-fixed etk-default-layout"
    :class="{ 'layout-vertical-nav-collapsed': collapsed, 'layout-horizontal-nav': horizontal,
      'layout-horizontal-nav-active': horizontal, 'layout-navbar-floating-eligible': horizontal,
      'layout-navbar-away-from-top': navbarAwayFromTop, 'layout-horizontal-nav-scrolled': horizontal && navbarAwayFromTop,
      'layout-app-shell': mdAndDown, 'layout-window-controls-overlay-shell': isWindowControlsOverlayMode,
      'layout-fixed-shell-backplate-active': showFixedBackplate }"
    :style="{ '--shell-floating-navbar-scale-x': floatingNavbarScale }"
    :data-shell-mode="mdAndDown ? 'app' : 'desktop'" :data-shell-display-environment="displayEnvironment"
    :data-shell-navbar-attachment="horizontal ? 'theme-qualified' : 'connected'"
    :data-glass-navigation-refraction="supportsRefraction ? 'chromium' : undefined"
    :data-glass-navbar-refraction="supportsRefraction ? 'chromium' : undefined">
    <GlassNavbarRefractionDefs v-if="supportsRefraction" :key="`navbar-${glassShellKey}`" />
    <GlassPanelRefractionDefs v-if="supportsRefraction" :key="`panel-${glassShellKey}`" />
    <GlassFixedShellBackplate v-if="showFixedBackplate" :is-overlay-nav="mdAndDown" :is-overlay-nav-active="overlayOpen"
      :layers="fixedShell.layers.value" :transition-duration-ms="fixedShell.transitionDurationMs" />
    <VNavigationDrawer v-if="!horizontal" :model-value="mdAndDown ? overlayOpen : true"
      :temporary="mdAndDown" :permanent="!mdAndDown" :rail="collapsed" :width="260" :rail-width="80"
      class="layout-vertical-nav" :class="{ 'overlay-nav': mdAndDown, visible: !mdAndDown || overlayOpen }"
      @update:model-value="overlayOpen = $event">
      <div class="nav-header">
        <slot name="brand" :rail="collapsed" />
        <VBtn v-if="mdAndDown" icon="mdi-close" variant="text" color="default" size="small" aria-label="关闭导航" @click="overlayOpen = false" />
      </div>
      <VList v-model:opened="opened" class="nav-items" color="primary" nav density="comfortable" open-strategy="multiple">
        <template v-for="menu in menus" :key="menu.key">
          <VListGroup v-if="menu.children" :value="menu.key">
            <template #activator="{ props: groupProps, isOpen }">
              <VListItem v-bind="groupProps" :title="menu.label" :prepend-icon="menu.icon" :aria-expanded="isOpen" />
            </template>
            <VListItem v-for="item in menu.children" :key="item.key" :title="item.label" :prepend-icon="item.icon"
              :active="item.key === activeKey" :value="item.key" @click="navigate(item.key)" />
          </VListGroup>
          <VTooltip v-else :text="menu.label" location="end" :disabled="!collapsed">
            <template #activator="{ props: tooltipProps }">
              <VListItem v-bind="tooltipProps" :title="menu.label" :prepend-icon="menu.icon"
                :active="menu.key === activeKey" :value="menu.key" @click="navigate(menu.key)" />
            </template>
          </VTooltip>
        </template>
      </VList>
    </VNavigationDrawer>

    <VAppBar class="layout-navbar" :class="{ 'layout-navbar--horizontal': horizontal }" flat
      :height="(horizontal ? 104 : 64) + (hasHeaderTabs ? 44 : 0)" :style="horizontal ? { '--shell-navbar-scroll-progress': scrollProgress } : undefined">
      <div class="navbar-content-container">
        <div class="theme-navbar-row">
          <slot v-if="horizontal" name="brand" :rail="false" />
          <VBtn v-if="mdAndDown" icon="mdi-menu" variant="text" color="default" aria-label="切换导航"
            :aria-expanded="overlayOpen" @click="toggleNavigation" />
          <slot name="navbar-leading" />
          <VSpacer />
          <div class="theme-navbar-actions"><slot name="navbar-actions" /></div>
        </div>
        <nav v-if="horizontal" class="theme-horizontal-nav" aria-label="主导航">
          <template v-for="group in menus" :key="group.key">
            <VMenu v-if="group.children" :model-value="openHorizontalGroup === group.key" location="bottom start"
              offset="8" open-on-hover :open-delay="0" :close-delay="120"
              @update:model-value="openHorizontalGroup = $event ? group.key : null">
              <template #activator="{ props: menuProps }">
                <VBtn v-bind="menuProps" :prepend-icon="group.icon" append-icon="mdi-chevron-down" rounded="pill"
                  :variant="groupActive(group) ? 'tonal' : 'text'" :color="groupActive(group) ? 'primary' : undefined"
                  class="theme-horizontal-nav__item">{{ group.label }}</VBtn>
              </template>
              <VList class="theme-horizontal-nav__menu" min-width="208" density="comfortable">
                <VListItem v-for="item in group.children" :key="item.key" :title="item.label" :prepend-icon="item.icon"
                  :active="item.key === activeKey" @click="navigate(item.key)" />
              </VList>
            </VMenu>
            <VBtn v-else :prepend-icon="group.icon" :variant="groupActive(group) ? 'tonal' : 'text'" rounded="pill"
              class="theme-horizontal-nav__item" @click="navigate(group.key)">{{ group.label }}</VBtn>
          </template>
        </nav>
        <slot name="header-tabs" />
      </div>
    </VAppBar>

    <VMain class="layout-page-wrapper">
      <main class="layout-page-content mp-page-content">
        <div class="page-content-container main-content-wrapper"><slot /></div>
      </main>
    </VMain>

    <Teleport v-if="mdAndDown" to="body">
      <nav class="footer-nav-container" aria-label="快捷导航">
        <VCard class="footer-nav-card" rounded="pill" elevation="3">
          <VCardText class="footer-card-content">
            <VBtn v-for="item in dockMenus" :key="item.key" variant="text" rounded="pill" class="footer-nav-btn"
              :color="item.key === activeKey ? 'primary' : undefined" :aria-label="item.label" @click="navigate(item.key)">
              <div class="btn-content"><VIcon :icon="item.icon" size="25" /><span>{{ item.label }}</span></div>
            </VBtn>
            <VBtn variant="text" rounded="pill" class="footer-nav-btn" aria-label="更多导航" @click="overlayOpen = true">
              <div class="btn-content"><VIcon icon="mdi-dots-horizontal" size="25" /><span>更多</span></div>
            </VBtn>
          </VCardText>
        </VCard>
      </nav>
    </Teleport>
  </VLayout>
</template>

<style lang="scss" scoped>
/* Browser adaptation of upstream DefaultLayout and Footer, using native Vuetify layout offsets. */
.etk-default-layout {
  --shell-floating-navbar-radius: var(--glass-v3-navigation-radius, 1rem);
  --shell-floating-navbar-inset: 1rem;
  --shell-floating-navbar-motion-duration: 150ms;
  --shell-floating-navbar-motion-easing: cubic-bezier(0.2, 0, 0, 1);
  min-block-size: 100dvh;
  background: transparent;
}
.nav-header { display: flex; align-items: center; justify-content: space-between; flex: 0 0 auto; min-block-size: 88px; padding: 20px; }
.layout-vertical-nav :deep(.v-navigation-drawer__content) { display: flex; flex-direction: column; }
.nav-items { flex: 1 0 auto; inline-size: 100%; padding: 8px 10px 24px; }
.nav-items :deep(.v-list-item) { min-block-size: 52px; margin-block-end: 6px; }
.nav-items :deep(.v-list-group) { margin-block-end: 12px; }
// VApp already owns a Vuetify layout. Its nested VLayout otherwise registers these as
// absolute items; retain Vuetify's offsets while attaching the shell to the viewport.
.layout-vertical-nav, .layout-navbar { position: fixed !important; }
.layout-vertical-nav { border-inline-end: 1px solid rgba(var(--v-theme-on-surface), .08); }
.nav-items :deep(.v-list-item-title) { font-size: 17px; line-height: 1.5; white-space: normal; overflow-wrap: anywhere; }
html[data-theme='glass'][data-glass-color-scheme='dark'] .nav-items :deep(.v-list-item--active),
html[data-theme='glass'][data-glass-color-scheme='dark'] .theme-horizontal-nav__item.text-primary,
html[data-theme='glass'][data-glass-color-scheme='dark'] .footer-nav-btn.text-primary {
  color: color-mix(in srgb, rgb(var(--v-theme-on-surface)) 95%, rgb(var(--v-theme-primary))) !important;
}
html[data-theme='glass'][data-glass-color-scheme='dark'] .theme-horizontal-nav__item.text-primary :deep(.v-btn__underlay) {
  background-color: rgb(var(--v-theme-primary));
}
.navbar-content-container { inline-size: 100%; padding-inline: 8px; padding-block-start: env(safe-area-inset-top); }
.theme-navbar-row, .theme-navbar-actions { display: flex; align-items: center; gap: 8px; min-inline-size: 0; }
.theme-navbar-row { min-block-size: 64px; }
.theme-navbar-actions { flex: 0 0 auto; }
.layout-navbar { border-block-end: 1px solid rgba(var(--v-theme-on-surface), .08); }
.layout-navbar--horizontal { border-radius: calc(var(--shell-navbar-scroll-progress, 0) * 24px); transform: translateY(calc(var(--shell-navbar-scroll-progress, 0) * 12px)); margin-inline: calc(var(--shell-navbar-scroll-progress, 0) * 16px); max-inline-size: calc(100% - var(--shell-navbar-scroll-progress, 0) * 32px); }
.theme-horizontal-nav { display: flex; align-items: center; gap: 4px; padding-block-end: 8px; }
.theme-horizontal-nav__item { letter-spacing: 0; padding-inline: 12px !important; }
.theme-horizontal-nav__menu { border: 1px solid rgba(var(--v-theme-on-surface), .08); }
// VMain already reserves the registered navbar; override the upstream raw-shell padding calculation.
.layout-page-content { inline-size: 100%; max-inline-size: none; padding: 24px 8px; padding-block-start: 24px !important; }
.main-content-wrapper { inline-size: 100%; min-inline-size: 0; }
.main-content-wrapper :deep(> div:first-child) { inline-size: 100% !important; min-inline-size: 0; }
.layout-app-shell .layout-page-content { padding-block-end: calc(96px + env(safe-area-inset-bottom)); }
.footer-nav-container { position: fixed; z-index: 1999; inset-block-end: 0; inset-inline: 0; display: flex; justify-content: center; padding-block-end: calc(6px + env(safe-area-inset-bottom)); pointer-events: none; }
.footer-nav-card { flex: 0 0 auto; overflow: hidden; border: 1px solid rgba(var(--v-theme-on-surface), .08); backdrop-filter: blur(24px); background-color: rgba(var(--v-theme-surface), .7); pointer-events: auto; }
.footer-card-content { display: flex; padding: 4px 6px; }
.footer-nav-btn { min-inline-size: 68px; block-size: 54px; padding-inline: 8px; }
.btn-content { display: flex; flex-direction: column; align-items: center; gap: 2px; }
.btn-content span { font-size: 10px; max-inline-size: 76px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
:global(html[data-theme-customizer-open='true'] .footer-nav-container) { visibility: hidden; pointer-events: none; }
@media (width <= 1280px) { .layout-page-content { padding: 16px 8px; padding-block-start: 16px !important; } }
@media (prefers-reduced-motion: reduce) { .layout-navbar--horizontal { transform: none; } }
</style>
