/// <reference types="vite/client" />

declare module "*.vue" {
  import type { DefineComponent } from "vue";
  const component: DefineComponent<{}, {}, any>;
  export default component;
}

// Tauri setup 阶段注入到 WebView 的全局函数：隐藏启动画面
interface Window {
  __hideSplash?: () => void;
}
