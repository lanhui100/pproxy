import { createPinia } from 'pinia'
import { createApp } from 'vue'

import App from './App.vue'
import router from './router'

import './assets/main.css'

// 单体化收尾：admin token 与远端管理面已废弃，桌面端不再连接任何后端。
// 清理旧架构在浏览器 dev / WebView 里残留的后端地址，防止任何残留路径误连。
if (typeof localStorage !== 'undefined') {
  localStorage.removeItem('pony-backend-url')
  localStorage.removeItem('pony-dev-admin-token')
}

createApp(App).use(createPinia()).use(router).mount('#app')
