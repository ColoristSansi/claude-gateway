import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [react()],
  // src-tauri 的 devUrl 指到 1420；端口被占就直接失败，不静默换端口。
  server: { port: 1420, strictPort: true },
  build: {
    // shortcut: 不清空 dist，保住 app/dist/.gitkeep（tauri::generate_context! 编译期要求
    // frontendDist 目录存在）；改成先清 dist 的构建流程时删掉这行。
    emptyOutDir: false,
  },
});
