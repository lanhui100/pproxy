import { createRouter, createWebHistory } from 'vue-router'

// Tauri v2 Windows 下以 http://tauri.localhost 提供页面，history 模式可用
const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', name: 'dashboard', component: () => import('@/views/DashboardView.vue') },
    { path: '/core', name: 'core', component: () => import('@/views/CoreView.vue') },
    { path: '/settings', name: 'settings', component: () => import('@/views/SettingsView.vue') },
    // 兼容历史路径重定向
    { path: '/proxy', redirect: '/core' },
    { path: '/routes', redirect: '/core' },
    { path: '/tokens', redirect: '/settings' },
    { path: '/usage', redirect: '/' },
  ],
})

export default router
