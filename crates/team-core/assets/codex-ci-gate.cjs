'use strict';
const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const VALUE_FLAGS = new Set(['-b', '--body', '-F', '--body-file', '-t', '--subject', '-A', '--author-email', '--match-head-commit']);

function findMerges(command, cwd = process.cwd()) {
  const tokens = String(command).match(/"(?:[^"\\]|\\"|\\(?!"))*"|'(?:[^']|'')*'|&&|\|\||[;\n&|()]|[^\s;&|()]+/g) || [];
  let dir = cwd, segment = [];
  const merges = [];
  function inspect() {
    const words = segment.map(s => s.replace(/^(["'])(.*)\1$/s, '$2'));
    segment = [];
    if (['cd', 'Set-Location'].includes(words[0])) {
      const target = words.slice(1).filter(w => !['/d', '-Path', '-LiteralPath'].includes(w))[0];
      if (target && !/[$`]/.test(target)) dir = path.resolve(dir, target);
      return;
    }
    if (words[0] === 'command') words.shift();
    if (!/(^|[/\\])gh(?:\.exe)?$/i.test(words[0] || '')) return;
    const rest = words.slice(1);
    let repo = null;
    while (['-R', '--repo'].includes(rest[0]) || rest[0]?.startsWith('--repo=')) {
      const flag = rest.shift();
      repo = flag.includes('=') ? flag.slice(7) : rest.shift();
    }
    if (rest[0] === 'pr' && rest[1] === 'merge') {
      const merge = { target: null, repo, auto: false, cwd: dir };
      for (let i = 2; i < rest.length; i++) {
        const arg = rest[i];
        if (['--auto', '--disable-auto'].includes(arg)) merge.auto = true;
        else if (['-R', '--repo'].includes(arg)) merge.repo = rest[++i];
        else if (arg.startsWith('--repo=')) merge.repo = arg.slice(7);
        else if (VALUE_FLAGS.has(arg)) i++;
        else if (!arg.startsWith('-') && merge.target === null) merge.target = arg;
      }
      merges.push(merge);
    } else if (rest[0] === 'api') {
      const endpoint = rest.find(t => /^\/?repos\/[^/]+\/[^/]+\/pulls\/\d+\/merge$/.test(t));
      const put = rest.some((t, i) => /^(?:-X|--method)=?PUT$/i.test(t) || (['-X', '--method'].includes(t) && /^PUT$/i.test(rest[i + 1] || '')));
      if (endpoint && put) {
        const [, owner, name, number] = endpoint.match(/^\/?repos\/([^/]+)\/([^/]+)\/pulls\/(\d+)\/merge$/);
        merges.push({ target: number, repo: `${owner}/${name}`, auto: false, cwd: dir });
      }
    }
  }
  for (const token of tokens) {
    if (/^(?:&&|\|\||[;\n&|()])$/.test(token)) inspect(); else segment.push(token);
  }
  inspect();
  return merges;
}

function checksFor(merge, execute = spawnSync, timeout = 25000) {
  const label = `${merge.repo || ''} PR ${merge.target || '(current branch)'}`.trim();
  const args = ['pr', 'checks'];
  if (merge.target) args.push(merge.target);
  if (merge.repo) args.push('--repo', merge.repo);
  args.push('--json', 'name,state,bucket');
  const result = execute(process.env.LUNIMA_GH_EXECUTABLE || 'gh', args, {
    cwd: merge.cwd, encoding: 'utf8', timeout, windowsHide: true, maxBuffer: 1024 * 1024,
  });
  if (result.error) return { label, error: 'gh checks unavailable or timed out' };
  if (/no checks reported/i.test(result.stderr || '')) return { label, checks: [] };
  try {
    const checks = JSON.parse(result.stdout || '');
    if (![0, 8].includes(result.status) || !Array.isArray(checks)) throw Error('unreadable');
    return { label, checks };
  } catch { return { label, error: 'gh did not return successful CI check data' }; }
}

function decide(results) {
  const problems = [];
  for (const result of results) {
    if (result.error) problems.push(`${result.label}: ${result.error}`);
    else if (!result.checks.length) problems.push(`${result.label}: no CI checks reported`);
    else for (const check of result.checks) {
      if (!['pass', 'skipping'].includes(check?.bucket)) {
        problems.push(`${result.label}: ${check?.name || 'check'} (${check?.bucket || 'unknown'})`);
      }
    }
  }
  return problems.length ? { permissionDecision: 'deny', permissionDecisionReason:
    `CI gate: merge blocked; CI is not green.\n${problems.join('\n')}\nWait for pending checks; investigate failures or retry transient failures once. Missing, failed, cancelled, or unreadable checks require human review. Do not bypass this gate. gh pr merge --auto delegates required checks to GitHub.` } : null;
}

function handle(event, query = checksFor) {
  if (event.hook_event_name !== 'PreToolUse') return null;
  const merges = findMerges(event.tool_input?.command ?? event.tool_input?.cmd ?? '', event.cwd).filter(m => !m.auto);
  const deadline = Date.now() + 25000;
  const results = merges.map(merge => {
    if (Date.now() >= deadline) return { label: 'PR', error: 'CI gate time budget exhausted' };
    return query(merge, undefined, Math.max(1, deadline - Date.now()));
  });
  const verdict = decide(results);
  return verdict ? { hookSpecificOutput: { hookEventName: 'PreToolUse', ...verdict } } : null;
}

module.exports = { findMerges, checksFor, decide, handle };
if (require.main === module || (!module.parent && !process.argv[1])) {
  try {
    const output = handle(JSON.parse(fs.readFileSync(0, 'utf8').replace(/^\uFEFF/, '')));
    if (output) process.stdout.write(JSON.stringify(output));
  } catch {
    process.stdout.write(JSON.stringify({ hookSpecificOutput: { hookEventName: 'PreToolUse',
      permissionDecision: 'deny', permissionDecisionReason: 'CI gate could not evaluate this command; resolve the hook error before retrying.' } }));
  }
}
