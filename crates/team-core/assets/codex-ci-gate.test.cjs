const { test } = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const { findMerges, decide, handle, checksFor } = require('./codex-ci-gate.cjs');
test('finds CLI and REST merges with repository and cwd', () => {
  const cwd = process.cwd();
  const merges = findMerges('cd "other repo" && gh pr merge 42 --repo owner/repo --squash', cwd);
  assert.equal(merges[0].target, '42');
  assert.equal(merges[0].repo, 'owner/repo');
  assert.equal(merges[0].cwd, path.resolve(cwd, 'other repo'));
  assert.equal(findMerges('gh api --method=PUT repos/owner/repo/pulls/9/merge', cwd)[0].target, '9');
  assert.equal(findMerges('gh -R owner/repo pr merge 42', cwd)[0].repo, 'owner/repo');
  for (const command of ['echo "gh pr merge 42"', 'gh pr view 42', 'gh api repos/a/b/pulls/9/merge']) {
    assert.deepEqual(findMerges(command, cwd), []);
  }
});
test('only pass and skipping checks allow immediate merges', () => {
  for (const bucket of ['pass', 'skipping']) assert.equal(decide([{ label: 'PR', checks: [{ bucket }] }]), null);
  for (const bucket of ['pending', 'fail', 'cancel', 'unknown', undefined]) {
    assert.equal(decide([{ label: 'PR', checks: [{ name: 'build', bucket }] }]).permissionDecision, 'deny');
  }
  for (const result of [{ checks: [] }, { error: 'unavailable' }]) {
    assert.equal(decide([{ label: 'PR', ...result }]).permissionDecision, 'deny');
  }
});
test('checks only invokes gh pr checks, handles JSON failures and missing gh', () => {
  const merge = { target: '42', repo: 'owner/repo', cwd: process.cwd() };
  const result = checksFor(merge, (_, args) => {
    assert.deepEqual(args, ['pr', 'checks', '42', '--repo', 'owner/repo', '--json', 'name,state,bucket']);
    return { status: 8, stdout: '[{"name":"build","bucket":"pending"}]' };
  });
  assert.equal(result.checks[0].bucket, 'pending');
  assert.ok(checksFor(merge, () => ({ error: Error('missing') })).error);
  assert.ok(checksFor(merge, () => ({ status: 1, stdout: 'bad JSON' })).error);
  assert.ok(checksFor(merge, () => ({ status: 1, stdout: '[]' })).error);
});
test('auto merge and unrelated tools skip checks; failed CI emits native denial', () => {
  const event = command => ({ hook_event_name: 'PreToolUse', cwd: process.cwd(), tool_input: { command } });
  for (const command of ['gh pr merge 42 --auto', 'gh pr merge 42 --disable-auto', 'git status']) {
    assert.equal(handle(event(command), () => { throw Error('must not query'); }), null);
  }
  const output = handle(event('gh pr merge 42'), () => ({ label: 'PR 42', checks: [{ bucket: 'fail', name: 'build' }] }));
  assert.equal(output.hookSpecificOutput.hookEventName, 'PreToolUse');
  assert.equal(output.hookSpecificOutput.permissionDecision, 'deny');
});
