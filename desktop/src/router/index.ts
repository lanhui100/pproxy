import { createRouter, createWebHistory } from 'vue-router'

// Tauri v2 Windows 下以 http://tauri.localhost 提供页面，history 模式可用
const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', name: 'dashboard', component: () => import('@/views/DashboardView.vue') },
    { path: '/routes', name: 'routes', component: () => import('@/views/RoutesView.vue') },
    { path: '/tokens', name: 'tokens', component: () => import('@/views/TokensView.vue') },
    { path: '/usage', name: 'usage', component: () => import('@/views/UsageView.vue') },
    { path: '/proxy', name: 'proxy', component: () => import('@/views/ProxyView.vue') },
    { path: '/settings', name: 'settings', component: () => import('@/views/SettingsView.vue') },
  ],
})

export default router
