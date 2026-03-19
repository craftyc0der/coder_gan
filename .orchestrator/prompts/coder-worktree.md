
=== WORKTREE MODE — GIT WORKFLOW ===

You are working in a git worktree with your own dedicated branch.

YOUR BRANCH: {{my_branch}}
YOUR WORKTREE: {{worktree_root}}

Other agents and their branches:
{{other_branches}}

CRITICAL RULES FOR WORKTREE MODE:

1. COMMIT YOUR WORK. You are on your own branch. You MUST `git add` and
   `git commit` your changes frequently — at minimum after every logical
   unit of work. Uncommitted code is invisible to the reviewer and other
   agents. Small, frequent commits are better than one large commit.

2. INCLUDE YOUR BRANCH IN EVERY MESSAGE. When you send a message to any
   other agent, always include the line:
     BRANCH: {{my_branch}}
   This tells the recipient where to find your code. Without this, they
   cannot review or test your work.

3. MERGE THE REVIEWER'S BRANCH BEFORE STARTING. Before you begin any new
   work, pull in the latest approved code from the reviewer's branch:
     git merge <reviewer-branch> --no-edit
   If there are merge conflicts, resolve them, commit, and then proceed.
   The reviewer's branch contains the accepted, tested codebase. Starting
   from it ensures you are building on approved work, not stale code.

4. WHEN TO MERGE AGAIN. Any time the reviewer tells you they have approved
   and committed new work, merge their branch again before continuing.

=== WORKFLOW SUMMARY ===

  START:   git merge <reviewer-branch> --no-edit
  WORK:    write code → git add -A → git commit -m "description"
  NOTIFY:  send message including BRANCH: {{my_branch}}
  REPEAT:  merge reviewer branch when told there are new approvals
