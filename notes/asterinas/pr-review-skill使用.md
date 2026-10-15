How to use the `pr-review` skill

[https://github.com/tatetian/asterinas/tree/pr-review-skill](https://github.com/tatetian/asterinas/tree/pr-review-skill)

**1. Pull the skill files into their working tree** (no branch switch, no history rewrite):

```bash

git fetch  [https://github.com/tatetian/asterinas](https://github.com/tatetian/asterinas)  pr-review-skill

git checkout FETCH_HEAD -- .agents/skills/pr-review .claude/skills

```

This drops `.agents/skills/pr-review/` and the `.claude/skills` symlink into their repo. They can commit it or keep it local.

**2. Verify `gh` is ready** (the skill shells out to it):

```bash

gh auth status

```

If not authenticated: `gh auth login`.

**3. Use it inside Claude Code** (from the repo root):

```

/pr-review new      # generate a review

/pr-review submit          # post comments to GitHub

/pr-review redo            # re-review after PR updates

/pr-review delete          # clean up worktree + review files

```

Reviews land in `pr_reviews//review-.md` with a convenience symlink at `pr_reviews/.md`.