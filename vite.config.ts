import tailwindcss from '@tailwindcss/vite'
import { tanstackRouter } from '@tanstack/router-plugin/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [
    tanstackRouter({
      target: 'react',
      routesDirectory: 'src/routes',
      generatedRouteTree: 'src/routeTree.gen.ts',
      autoCodeSplitting: true,
    }),
    react(),
    tailwindcss(),
  ],
  resolve: {
    // Vite 8 native tsconfig `paths` resolution — the `@/*` alias comes straight
    // from tsconfig.json, with no manual mirror to keep in sync.
    tsconfigPaths: true,
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: '127.0.0.1',
  },
  // Pre-bundle every Tauri module the app touches: a mid-session re-optimize forces
  // a full-page reload, which in a desktop window is a visible flash.
  optimizeDeps: {
    include: ['@tauri-apps/api/core', '@tauri-apps/api/event', '@tauri-apps/plugin-opener'],
  },
  build: {
    // Tauri's real floor is the platform webview: evergreen WebView2 on Windows
    // (chrome105 per the official template), WKWebView on macOS and WebKitGTK on
    // Linux (safari15 covers both).
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari15',
  },
  envPrefix: ['VITE_', 'TAURI_'],
})
