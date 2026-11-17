const { test } = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const { candidates, handle, reviewerArgs, run } = require('./codex-comment-hook.cjs');

const cwd = process.cwd();
const event = (command, response = 'Process exited with code 0\n[main abc1234] change') => ({
  hook_event_name: 'PostToolUse', tool_name: 'Bash', cwd, model: 'gpt-test',
  tool_input: { command }, tool_response: response,
});

test('matches actual commands, not prose or other git operations', () => {
  for (const command of ['git status', 'echo "git commit -m hi"', '# git commit',
    'git log --grep="git commit"', 'printf "git commit"', 'git commit-tree HEAD']) {
    assert.deepEqual(candidates(command, cwd), [], command);
  }
  for (const command of ['git commit -m hi', 'git add . && git commit --amend --no-edit',
    '& git commit -m "hello; world"', 'git -c core.editor=true commit -m hi']) {
    assert.equal(candidates(command, cwd).length, 1, command);
  }
});

test('resolves cd, quoted paths and git -C without executing shell input', () => {
  assert.deepEqual(candidates('cd "other repo" && git -C nested commit -m hi', cwd),
    [path.resolve(cwd, 'other repo', 'nested')]);
  assert.deepEqual(candidates('Set-Location -LiteralPath "other repo"; git commit -m hi', cwd),
    [path.resolve(cwd, 'other repo')]);
});

test('skips failed commits, dry runs, unrelated events and recursion', async () => {
  const review = () => { throw Error('must not review'); };
  for (const e of [event('git commit', 'Process exited with code 1\nnothing to commit'),
    event('git commit || true', 'Process exited with code 0\nnothing to commit'),
    event('git commit --dry-run'), event('git status'),
    { ...event('git commit'), hook_event_name: 'PreToolUse' }]) {
    assert.equal(await handle(e, { review }), null);
  }
  assert.equal(await handle(event('git commit'), { recursive: true, review }), null);
});

test('reviews receipt SHA and sends actionable blocking feedback', async () => {
  const result = await handle(event('git commit -m hi'), { review: async (repo, sha, model) => {
    assert.equal(repo, cwd); assert.equal(sha, 'abc1234'); assert.equal(model, 'gpt-test');
    return { violations: ['src/main.rs:9: // increment counter'] };
  } });
  assert.equal(result.decision, 'block');
  assert.match(result.reason, /src\/main.rs:9/);
  assert.match(result.reason, /git commit --amend/);
  assert.equal(result.hookSpecificOutput.hookEventName, 'PostToolUse');
});

test('quiet successful commits review HEAD; failures are never reported as passes', async () => {
  assert.equal(await handle(event('git commit -qm hi', 'Exit code: 0'), {
    review: async (_, sha) => { assert.equal(sha, 'HEAD'); return { violations: [] }; },
  }), null);
  const result = await handle(event('git commit'), { review: async () => { throw Error('timed out'); } });
  assert.equal(result.decision, 'block');
  assert.match(result.reason, /could not complete.*timed out/s);
});

test('falls back to the parent model when the event omits it', async () => {
  const previous = process.env.LUNIMA_CODEX_MODEL;
  process.env.LUNIMA_CODEX_MODEL = 'parent-model';
  try {
    assert.equal(await handle({ ...event('git commit'), model: undefined }, {
      review: async (_, __, model) => { assert.equal(model, 'parent-model'); return { violations: [] }; },
    }), null);
  } finally {
    if (previous === undefined) delete process.env.LUNIMA_CODEX_MODEL;
    else process.env.LUNIMA_CODEX_MODEL = previous;
  }
});

test('reviewer is read-only, same-model, ephemeral, and disables hooks', () => {
  const args = reviewerArgs('schema.json', 'result.json', 'gpt-test', {});
  assert.ok(args.includes('read-only'));
  assert.ok(args.includes('features.hooks=false'));
  assert.ok(args.includes('gpt-test'));
  assert.ok(args.includes('--ephemeral'));
  assert.ok(args.includes('approval_policy="never"'));
});

test('API review uses the configured endpoint and model without leaking or falling back', () => {
  const env = { LUNIMA_CODEX_API_BASE_URL: 'https://example.test/v1', LUNIMA_CODEX_API_KEY: 'test-secret' };
  const args = reviewerArgs('schema', 'result', 'custom-model', env);
  assert.ok(args.includes('model_provider="lunima_api"'));
  assert.ok(!args.some(a => a.startsWith('forced_login_method=')));
  assert.ok(args.includes('custom-model'));
  assert.ok(args.includes('model_providers.lunima_api.base_url="https://example.test/v1"'));
  assert.ok(!args.join(' ').includes('test-secret'));
  for (const invalid of [{ LUNIMA_CODEX_API_BASE_URL: env.LUNIMA_CODEX_API_BASE_URL },
    { LUNIMA_CODEX_API_KEY: 'test-secret' }, { ...env, LUNIMA_CODEX_API_KEY: '' }]) {
    assert.throws(() => reviewerArgs('schema', 'result', 'custom-model', invalid), /fallback is disabled/);
  }
  assert.throws(() => reviewerArgs('schema', 'result', '', env), /fallback is disabled/);
});

test('reviewer preserves the validated parent credential store and defaults to file', () => {
  for (const store of ['file', 'keyring', 'auto', 'ephemeral']) {
    const args = reviewerArgs('schema', 'result', 'model', { LUNIMA_CODEX_AUTH_STORE: store });
    const index = args.indexOf(`cli_auth_credentials_store="${store}"`);
    assert.ok(index > 0, store);
    assert.equal(args[index - 1], '-c');
  }
  assert.ok(reviewerArgs('schema', 'result', 'model', {}).includes('cli_auth_credentials_store="file"'));
  for (const store of ['', 'unknown', 'FILE', 'file"\nmodel_provider="other']) {
    assert.throws(() => reviewerArgs('schema', 'result', 'model', { LUNIMA_CODEX_AUTH_STORE: store }),
      /Invalid LUNIMA_CODEX_AUTH_STORE/);
  }
});

test('process runner handles spaces, nonzero exit and timeout', async () => {
  assert.equal((await run(process.execPath, ['-e', 'process.stdout.write(process.argv[1])', 'a b'], { cwd })).trim(), 'a b');
  await assert.rejects(run(process.execPath, ['-e', 'process.exit(7)'], { cwd }), /exit 7/);
  await assert.rejects(run(process.execPath, ['-e', 'setInterval(()=>{},1000)'], { cwd, timeout: 100 }), /timed out/);
});

test('requiring the module from node -e does not run the stdin entry point', async () => {
  const modulePath = path.join(__dirname, 'codex-comment-hook.cjs');
  assert.equal(await run(process.execPath, ['-e', `require(${JSON.stringify(modulePath)})`], { cwd }), '');
});
