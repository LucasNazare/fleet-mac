import { defineConfig } from "@playwright/test";

// Runs the React UI in a plain browser against the in-memory mock backend (src/mockBackend.ts).
export default defineConfig({
  testDir: "tests/e2e",
  timeout: 20_000,
  use: { baseURL: "http://localhost:1420", viewport: { width: 1440, height: 900 } },
  webServer: { command: "npm run dev", port: 1420, reuseExistingServer: true },
  projects: [{ name: "webkit", use: { browserName: "webkit" } }],
});
