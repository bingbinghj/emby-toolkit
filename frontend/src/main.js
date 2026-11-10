import { createApp } from 'vue'
import { createPinia } from 'pinia'
import { createI18n } from 'vue-i18n'
import Toast from 'vue-toastification'
import 'vue-toastification/dist/index.css'
import { PerfectScrollbarPlugin } from 'vue3-perfect-scrollbar'
import 'vue3-perfect-scrollbar/style.css'
import { addCollection } from '@iconify/vue'
import App from './App.vue'
import router from './router'
import vuetify from './plugins/vuetify'
import UiIcon from './components/UiIcon.vue'
import zhCN from './locales/zh-CN'
import icons from './plugins/icons.json'
import { useAuthStore } from './stores/auth'
import { installBackgroundTaskInterceptor } from './utils/backgroundTasks'
import { initializeThemeRuntime } from './theme'
import { loadMoviePilotFonts } from './plugins/fonts'
import './styles/main.scss'
import './styles/etk.css'

icons.forEach(collection => addCollection(collection))
loadMoviePilotFonts()
const app = createApp(App)
app.use(createPinia())
app.use(vuetify)
app.use(createI18n({ legacy: false, locale: 'zh-CN', fallbackLocale: 'zh-CN', messages: { 'zh-CN': zhCN } }))
app.use(Toast, { position: 'bottom-right', hideProgressBar: true })
app.use(PerfectScrollbarPlugin)
app.component('UiIcon', UiIcon)
installBackgroundTaskInterceptor()
const disposeTheme = initializeThemeRuntime()

async function initializeApp() {
  const authStore = useAuthStore()
  try {
    await authStore.checkAuthStatus()
  } catch (error) {
    console.error('应用初始化失败：', error.message)
  } finally {
    app.use(router)
    app.mount('#app')
  }
}

void initializeApp()
if (import.meta.hot) import.meta.hot.dispose(disposeTheme)
