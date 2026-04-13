#!/usr/bin/env node
/**
 * bin/run.mjs — thin shim that delegates to the downloaded warpdrive-mcp binary.
 * With no arguments, launches the interactive setup wizard instead.
 */

import { spawn } from 'child_process';
import { fileURLToPath } from 'url';
import path from 'path';
import { existsSync } from 'fs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const args = process.argv.slice(2);
if (args.length === 0 && process.env.WARPDRIVE_SKIP_SETUP !== '1') {
  const { default: runSetup } = await import('./setup.mjs');
  await runSetup();
  process.exit(0);
}

const binaryName = process.platform === 'win32' ? 'warpdrive-mcp.exe' : 'warpdrive-mcp';
const binaryPath = path.join(__dirname, binaryName);

if (!existsSync(binaryPath)) {
  console.error(
    `warpdrive-mcp binary not found at ${binaryPath}.\n` +
    `Run: npm install -g @warpdrive/mcp  (to trigger postinstall)\n` +
    `Or build from source: cargo build --release -p warpdrive-mcp`
  );
  process.exit(1);
}

const proc = spawn(binaryPath, args, { stdio: 'inherit' });
proc.on('exit', (code) => process.exit(code ?? 0));
proc.on('error', (err) => {
  console.error(`Failed to start warpdrive-mcp: ${err.message}`);
  process.exit(1);
});
