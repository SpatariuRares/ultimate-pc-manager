# 0002. Public text describes only this app

Status: accepted

## Context

Code comments, documentation, commit messages and issues are public once the
repository is public. Text that names or compares other products ages badly, can
mislead readers about what this app does, and mixes this project's decisions with
someone else's.

## Decision

Every public text (code comments, docs, commit messages, issues, pull requests)
describes only this app, in our own words. It states what the app does and why. It
does not name other products or compare the app with them, and it does not copy
their wording. Names of operating systems, libraries, protocols and tools the app
depends on or talks to are fine: they are part of describing this app.

## Consequences

- Reviewers reject public text that names or compares other products.
- Requirements and behaviour are written as what this app does for its users.
- Existing text that does not follow this is fixed when it is touched.
