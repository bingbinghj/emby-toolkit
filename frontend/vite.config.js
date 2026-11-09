// vite.config.js
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import { VitePWA } from 'vite-plugin-pwa'
import vuetify from 'vite-plugin-vuetify'
import AutoImport from 'unplugin-auto-import/vite'
import { fileURLToPath, URL } from 'node:url'
import { version } from './package.json'

export default defineConfig({
  plugins: [
    vue(),
    vuetify({ autoImport: false, styles: { configFile: 'src/styles/variables/_vuetify.scss' } }),
    AutoImport({ imports: ['vue', 'vue-router', '@vueuse/core', 'pinia', 'vue-i18n'], vueTemplate: true, dts: false }),
    VitePWA({
      registerType: 'autoUpdate',
      includeAssets: ['favicon.ico', 'apple-touch-icon.png'],
      manifest: {
        name: 'Emby Toolkit',
        short_name: 'Emby Toolkit',
        description: 'Emby 媒体服务器增强管理工具',
        theme_color: '#8D51F9',
        background_color: '#171B20',
        display: 'standalone',
        start_url: '/',
        icons: [
          {
            src: 'pwa-192x192.png',
            sizes: '192x192',
            type: 'image/png'
          },
          {
            src: 'pwa-512x512.png',
            sizes: '512x512',
            type: 'image/png'
          },
          {
            src: 'pwa-512x512.png',
            sizes: '512x512',
            type: 'image/png',
            purpose: 'maskable'
          }
        ]
      },
      workbox: {
        maximumFileSizeToCacheInBytes: 4 * 1024 * 1024,
        globPatterns: ['**/*.{js,css,html,ico,png,svg,woff2,ttf,otf}'],
        navigateFallback: '/index.html',
        importScripts: ['cleanup-api-cache.js'],
        runtimeCaching: [
          {
            urlPattern: /^https?:\/\/.*\/api\/.*/i,
            handler: 'NetworkOnly',
            options: { fetchOptions: { cache: 'no-store' } },
          }
        ]
      }
    })
  ],
  define: {
    __APP_VERSION__: JSON.stringify(version)
  },
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
      '@core': fileURLToPath(new URL('./src/@core', import.meta.url)),
      '@layouts': fileURLToPath(new URL('./src/@layouts', import.meta.url)),
      '@styles': fileURLToPath(new URL('./src/styles', import.meta.url)),
      '@configured-variables': fileURLToPath(new URL('./src/styles/variables/_template.scss', import.meta.url)),
    },
  },
  build: {
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (!id.includes('node_modules')) return;
          if (id.includes('/three/')) return 'three';
          if (id.includes('/vuetify/')) return 'vuetify';
          if (id.includes('/echarts/')) return 'charts';
        },
      },
    },
  },
  server: {
    proxy: {
      // API 代理保持不变
      '/api': {
        target: 'http://localhost:5257',
        changeOrigin: true,
      },

      // ★★★ START: 3. 新增对 /image_proxy 的代理 ★★★
      // 这个规则专门用于代理图片请求
      '/image_proxy': {
        target: 'http://localhost:5257', // ETK Rust 后端
        changeOrigin: true,
        // 这里不需要路径重写，因为后端的路由就是 /image_proxy/...
      }
      // ★★★ END: 3. ★★★
    }
  }
})
