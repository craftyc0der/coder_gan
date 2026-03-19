
=== WORKTREE MODE — GIT WORKFLOW ===

You are working in a git worktree with your own dedicated branch.

YOUR BRANCH: {{my_branch}}
YOUR WORKTREE: {{worktree_root}}

Other agents and their branches:
{{other_branches}}

CRITICAL RULES FOR WORKTREE MODE:

1. COMMIT YOUR WORK. You are on your own branch. You MUST `git add` and
   `git commit` your test files after writing or updating them. Uncommitted
   tests are invisible to the reviewer.

2. INCLUDE YOUR BRANCH IN EVERY MESSAGE. When you send a message to any
   other agent, always include the line:
     BRANCH: {{my_branch}}
   This tells the recipient where to find your tests.

3. MERGE THE REVIEWER'S BRANCH BEFORE STARTING. Before you begin any new
   work, pull in the latest approved code from the reviewer's branch:
     git fetch origin
     git merge origin/<reviewer-branch> --no-edit
   The reviewer's branch has the accepted implementation code. You need it
   so your tests can compile and run against the actual source. If there
   are merge conflicts, resolve them and commit before proceeding.

4. MERGE THE CODER'S BRANCH WHEN TESTING. When the coder sends you a
   message asking you to test their work, they will include their branch
   name. Merge their branch to get their implementation code:
     git fetch origin
     git merge origin/<coder-branch> --no-edit
   Then write and run your tests against it.

5. WHEN TO MERGE AGAIN. Any time the reviewer tells you there are new
   approvals, merge the reviewer's branch before continuing.

=== WORKFLOW SUMMARY ===

  START:   git merge origin/<reviewer-branch> --no-edit
  TEST:    git merge origin/<coder-branch> --no-edit
           write tests → run tests → git add -A → git commit -m "description"
  NOTIFY:  send message including BRANCH: {{my_branch}}
  REPEAT:  merge reviewer branch when told there are new approvals
