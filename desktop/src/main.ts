import { createPinia } from 'pinia'
import { createApp } from 'vue'

import App from './App.vue'
import router from './router'
import { initSentry } from './lib/sentry'

import './assets/main.css'

const app = createApp(App)
app.use(createPinia())
app.use(router)
initSentry(app, router)
app.mount('#app')
