const marker = '<!-- tusklet-pr-builds -->';
const platforms = [
  ['macos-arm64', 'macOS · Apple Silicon', 'DMG'],
  ['windows-x64', 'Windows · x86-64', 'EXE installer'],
  ['linux-x64', 'Linux · x86-64', 'DEB + AppImage'],
];

function commentBody(run, artifacts, serverUrl, repo) {
  const runUrl = `${serverUrl}/${repo.owner}/${repo.repo}/actions/runs/${run.id}`;
  const completed = run.status === 'completed';
  const status = completed ? run.conclusion : 'building';
  const lines = [
    marker,
    `<!-- tusklet-build-run:${run.id}:${run.run_attempt} -->`,
    '## Test this PR',
    '',
    `Commit \`${run.head_sha.slice(0, 12)}\` · [Build #${run.run_number}, attempt ${run.run_attempt}](${runUrl}) · **${status}**`,
    '',
    'Installers are built from this PR merged with its base branch.',
    '',
    '| Platform | Package | Download |',
    '| --- | --- | --- |',
  ];

  for (const [platform, label, format] of platforms) {
    const artifact = artifacts.find((item) => item.name === `tusklet-${platform}`);
    let download = completed ? 'Unavailable — see build logs' : 'Building…';
    if (completed && artifact) {
      download = artifact.expired
        ? 'Expired — rerun the build'
        : `[Download](${runUrl}/artifacts/${artifact.id}) (expires ${artifact.expires_at.slice(0, 10)})`;
    }
    lines.push(`| ${label} | ${format} | ${download} |`);
  }

  if (run.conclusion === 'action_required') {
    lines.push('', 'This build needs approval in GitHub Actions before installers can be produced.');
  }

  lines.push(
    '',
    'Sign in to GitHub to download, then unzip the artifact to find the installer and its SHA-256 checksum. PR downloads are kept for 14 days; rerun **Package applications** to refresh them.',
    '',
    'These are preview builds. macOS builds are ad-hoc signed and not notarized; Windows installers are unsigned. Linux requires Ubuntu 24.04 or compatible (glibc 2.39+); AppImage also needs FUSE 2. Docker must be installed and running to manage databases.',
  );
  return lines.join('\n');
}

module.exports = async function updateComment({ github, context, core }) {
  const repo = context.repo;
  // Read current API state so delayed in_progress events cannot replace a
  // finished build, and delayed completion events cannot replace a rerun.
  const { data: run } = await github.rest.actions.getWorkflowRun({
    ...repo,
    run_id: context.payload.workflow_run.id,
  });
  if (run.event !== 'pull_request' || run.path !== '.github/workflows/release.yml') {
    core.info('Ignoring a run outside the PR packaging workflow.');
    return;
  }
  if (!run.head_repository) {
    core.info('The build source repository is no longer available.');
    return;
  }

  // Fork runs can have an empty workflow_run.pull_requests array. Resolve the
  // PR through GitHub instead of trusting a PR number supplied by an artifact.
  const candidates = await github.paginate(github.rest.pulls.list, {
    ...repo,
    state: 'open',
    head: `${run.head_repository.owner.login}:${run.head_branch}`,
    per_page: 100,
  });
  const matchesRun = (pr) => pr.state === 'open'
    && pr.head.repo?.id === run.head_repository.id
    && pr.head.ref === run.head_branch
    && pr.head.sha === run.head_sha;
  const pullRequests = candidates.filter(matchesRun);
  if (pullRequests.length === 0) {
    core.info('No open PR still matches this build commit.');
    return;
  }

  const artifacts = run.status === 'completed'
    ? await github.paginate(github.rest.actions.listWorkflowRunArtifacts, {
      ...repo,
      run_id: run.id,
      per_page: 100,
    })
    : [];
  const body = commentBody(run, artifacts, context.serverUrl, repo);

  for (const pr of pullRequests) {
    const comments = await github.paginate(github.rest.issues.listComments, {
      ...repo,
      issue_number: pr.number,
      per_page: 100,
    });
    const comment = comments.find((item) => item.user.type === 'Bot'
      && item.user.login === 'github-actions[bot]'
      && item.body?.startsWith(marker));
    const previous = comment?.body.match(/<!-- tusklet-build-run:(\d+):(\d+) -->/);
    if (previous && (Number(previous[1]) > run.id
      || (Number(previous[1]) === run.id && Number(previous[2]) > run.run_attempt))) {
      core.info(`Skipping an older build for PR #${pr.number}.`);
      continue;
    }

    // A push or close may have happened while listing artifacts and comments.
    const { data: current } = await github.rest.pulls.get({
      ...repo,
      pull_number: pr.number,
    });
    if (!matchesRun(current) || comment?.body === body) continue;

    if (comment) {
      await github.rest.issues.updateComment({ ...repo, comment_id: comment.id, body });
    } else {
      await github.rest.issues.createComment({ ...repo, issue_number: pr.number, body });
    }
  }
};
