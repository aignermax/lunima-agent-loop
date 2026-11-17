'use strict';
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const zlib = require('node:zlib');
const { spawn, spawnSync } = require('node:child_process');
const { codexCommand } = require('./codex-comment-hook.cjs');

async function inspect(overrides, cwd, home) {
  const [program, ...prefix] = codexCommand();
  const child = spawn(program, [...prefix, 'app-server', ...overrides.flatMap(v => ['-c', v])], {
    cwd, env: { ...process.env, CODEX_HOME: home }, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'],
  });
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { child.kill(); reject(Error('hooks/list timed out')); }, 30000);
    let pending = '';
    const send = value => child.stdin.write(JSON.stringify(value) + '\n');
    child.stderr.on('data', () => {});
    child.on('error', reject);
    child.stdout.on('data', chunk => {
      pending += chunk;
      let end;
      while ((end = pending.indexOf('\n')) >= 0) {
        const line = pending.slice(0, end); pending = pending.slice(end + 1);
        let message; try { message = JSON.parse(line); } catch { continue; }
        if (message.id === 1) {
          send({ method: 'initialized', params: {} });
          send({ id: 2, method: 'hooks/list', params: { cwds: [cwd] } });
        }
        if (message.id === 2) {
          clearTimeout(timer);
          child.once('close', () => {
            if (message.error) reject(Error(JSON.stringify(message.error)));
            else resolve(message.result);
          });
          if (process.platform === 'win32') spawnSync('taskkill', ['/pid', String(child.pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' });
          else child.kill();
        }
      }
    });
    send({ id: 1, method: 'initialize', params: { clientInfo: { name: 'lunima-hook-test', version: '1' }, capabilities: { experimentalApi: true } } });
  });
}

function entries(value) {
  if (!value || typeof value !== 'object') return [];
  if (value.currentHash) return [value];
  return Object.values(value).flatMap(entries);
}

async function main() {
  for (const [asset, event, timeout, status] of [
    ['codex-comment-hook', 'PostToolUse', 300, 'Checking commit comments with Codex'],
    ['codex-ci-gate', 'PreToolUse', 30, 'Checking PR CI before merge'],
  ]) {
  const payload = zlib.gzipSync(fs.readFileSync(path.join(__dirname, `${asset}.cjs`))).toString('base64');
  const command = `node -e "eval(require('node:zlib').gunzipSync(Buffer.from('${payload}','base64')).toString())"`;
  const override = `hooks.${event}=[{matcher='^Bash$',hooks=[{type='command',command=${JSON.stringify(command)},timeout=${timeout},statusMessage='${status}'}]}]`;
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'lunima-hook-build-'));
  try {
    const result = entries(await inspect(['features.hooks=true', override], temp, temp));
    const entry = result.find(e => e.statusMessage === status);
    if (!entry) throw Error(`Hook not discovered: ${JSON.stringify(result)}`);
    const trusted = `hooks.state={${JSON.stringify(entry.key)}={enabled=true,trusted_hash=${JSON.stringify(entry.currentHash)}}}`;
    const verified = entries(await inspect(['features.hooks=true', override, trusted], temp, temp));
    if (!verified.some(e => e.key === entry.key && e.trustStatus === 'trusted')) throw Error('Explicit session trust did not take effect');
    console.log(JSON.stringify({ key: entry.key, hash: entry.currentHash, trust: 'trusted', commandBytes: command.length }));
    if (process.argv.includes('--write')) {
      fs.writeFileSync(path.join(__dirname, `${asset}.b64`), payload + '\n');
      fs.writeFileSync(path.join(__dirname, asset === 'codex-comment-hook' ? 'codex-hook-trust.json' : 'codex-ci-gate-trust.json'), JSON.stringify({ hash: entry.currentHash }, null, 2) + '\n');
    }
  } finally { fs.rmSync(temp, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 }); }
  }
}
module.exports = { inspect, entries };
if (require.main === module) main().catch(error => { console.error(error); process.exitCode = 1; });
