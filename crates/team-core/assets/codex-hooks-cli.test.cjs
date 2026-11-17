'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const http = require('node:http');
const { spawn } = require('node:child_process');
const { codexCommand } = require('./codex-comment-hook.cjs');

function run(program, args, options) {
  return new Promise((resolve, reject) => {
    const child = spawn(program, args, { cwd: options.cwd, env: options.env, windowsHide: true });
    let output = '', errors = '';
    child.stdout.on('data', chunk => { output += chunk; });
    child.stderr.on('data', chunk => { errors += chunk; });
    child.on('error', reject);
    child.on('close', code => code === 0 ? resolve(output) : reject(Error(output + errors)));
    child.stdin.end(options.input);
  });
}

for (const gate of [false, true]) {
test(`native CLI ${gate ? 'blocks a merge before execution' : 'returns commit review failure feedback'}`, async () => {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'lunima-codex-cli-'));
  const requests = [];
  const server = http.createServer((req, res) => {
    let body = '';
    req.on('data', chunk => { body += chunk; });
    req.on('end', () => {
      let request; try { request = JSON.parse(body); } catch { res.writeHead(404); res.end(); return; }
      requests.push(request);
      const tools = request.tools || [];
      const tool = tools.find(t => ['exec_command', 'shell_command', 'shell'].includes(t.name));
      const command = gate ? (process.platform === 'win32'
        ? "function gh { Write-Output 'MERGE_EXECUTED' }; gh pr merge 42 --repo fixture/no-network"
        : "gh() { printf 'MERGE_EXECUTED\\n'; }; gh pr merge 42 --repo fixture/no-network") : process.platform === 'win32'
        ? "function git { Write-Output '[test 0000000] simulated commit' }; git commit -m probe"
        : "git() { printf '[test 0000000] simulated commit\\n'; }; git commit -m probe";
      const args = tool?.name === 'exec_command' ? { cmd: command, max_output_tokens: 1000 }
        : tool?.name === 'shell' ? { command: ['sh', '-c', command] } : { command };
      const item = requests.length === 1 && tool
        ? { type: 'function_call', id: 'fc_probe', call_id: 'call_probe', name: tool.name, arguments: JSON.stringify(args) }
        : { type: 'message', id: 'msg_done', role: 'assistant', status: 'completed', content: [{ type: 'output_text', text: 'DONE', annotations: [] }] };
      res.writeHead(200, { 'Content-Type': 'text/event-stream' });
      for (const data of [
        { type: 'response.created', response: { id: `resp_${requests.length}`, status: 'in_progress' } },
        { type: 'response.output_item.added', output_index: 0, item },
        { type: 'response.output_item.done', output_index: 0, item },
        { type: 'response.completed', response: { id: `resp_${requests.length}`, status: 'completed', output: [item], usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 } } },
      ]) res.write(`event: ${data.type}\ndata: ${JSON.stringify(data)}\n\n`);
      res.end();
    });
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    const asset = gate ? 'codex-ci-gate' : 'codex-comment-hook';
    const payload = fs.readFileSync(path.join(__dirname, `${asset}.b64`), 'utf8').trim();
    const command = `node -e "eval(require('node:zlib').gunzipSync(Buffer.from('${payload}','base64')).toString())"`;
    const eventKey = gate ? 'pre_tool_use' : 'post_tool_use';
    const key = (process.platform === 'win32' ? 'C:\\<session-flags>\\config.toml:' : '/<session-flags>/config.toml:') + `${eventKey}:0:0`;
    const hash = JSON.parse(fs.readFileSync(path.join(__dirname, gate ? 'codex-ci-gate-trust.json' : 'codex-hook-trust.json'), 'utf8')).hash;
    const settings = ['features.hooks=true', 'model_provider="fixture"',
      'features.plugins=false', 'features.apps=false', 'features.code_mode=false', 'features.code_mode_only=false',
      'mcp_servers={}', 'approval_policy="never"',
      `model_providers.fixture={name='Fixture',base_url='http://127.0.0.1:${server.address().port}/v1',wire_api='responses',requires_openai_auth=false}`, 
      `hooks.${gate ? 'PreToolUse' : 'PostToolUse'}=[{matcher='^Bash$',hooks=[{type='command',command=${JSON.stringify(command)},timeout=${gate ? 30 : 300},statusMessage='${gate ? 'Checking PR CI before merge' : 'Checking commit comments with Codex'}'}]}]`,
      `hooks.state={${JSON.stringify(key)}={enabled=true,trusted_hash=${JSON.stringify(hash)}}}`];
    const [program, ...prefix] = codexCommand();
    const output = await run(program, [...prefix, 'exec', '--ignore-user-config', '--ephemeral', '--json',
      '--dangerously-bypass-approvals-and-sandbox', '--model', 'fixture', ...settings.flatMap(v => ['-c', v]), '-'], {
      cwd: process.cwd(), env: { ...process.env, CODEX_HOME: home, LUNIMA_GH_EXECUTABLE: path.join(home, 'nonexistent-gh') }, timeout: 45000,
      input: 'Run the supplied shell fixture. It simulates a git commit; it must never create a real commit.',
    });
    assert.ok(requests.length >= 2, `No shell request completed: ${output}`);
    assert.match(JSON.stringify(requests.slice(1)), gate ? /CI gate: merge blocked/ : /Commit comment review could not complete/);
    if (gate) {
      const events = output.trim().split('\n').map(line => JSON.parse(line));
      assert.ok(!events.some(e => (e.item?.aggregated_output || '').includes('MERGE_EXECUTED')));
    }
    assert.ok(!output.includes('not yet trusted'));
  } finally {
    await new Promise(resolve => server.close(resolve));
    fs.rmSync(home, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
});
}
