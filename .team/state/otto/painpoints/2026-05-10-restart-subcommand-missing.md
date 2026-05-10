# painpoint — `teamctl restart <project>` doesn't exist on 0.7.3

> Filed: 2026-05-10 by ops:otto
> Affects: ops workflow, role-doc fidelity

## Symptom

My role description (and §8 hard rules) reference `teamctl restart teamctl` as the canonical scoped-restart command. On the running binary (0.7.3), no such subcommand exists:

```
$ teamctl restart teamctl
error: unrecognized subcommand 'restart'
```

So when the project owner asks me to "restart the team," the documented path doesn't run. I had to fall back to:

```
teamctl sessions kill teamctl
teamctl up
```

That works — `sessions kill <project>` is properly scoped and `up` is idempotent for already-running sessions, so it does not disturb the `ops` project (i.e. me). But:

1. It's two commands instead of one, so a hurried op risks doing only the `kill` and leaving the dev team down.
2. Anyone reading the role doc as a script would hit a dead end on the first command.
3. `up` re-renders artifacts for *all* projects in the compose root, which is more side-effect than a clean restart should imply. There's no way to verify the ops side wasn't touched without grepping render output.

## What I'd want, if asked

A first-class `teamctl restart <project>` that is exactly equivalent to "kill that project's sessions, then bring just that project back up." Hard-scoped to one project — so the bare form is either an error or an explicit `--all`. That removes the entire foot-gun the role's hard rule §1 is trying to guard against by convention.

A weaker version: a `teamctl up <project>` flag that limits the up-scope to the named project, so the recipe stays two commands but neither one can spill outside the project.

## Not proposing the fix

Leaving the design call to the project owner / hugo. Surfacing as a painpoint per role §4 (write painpoints, don't invent the solution unilaterally). My role doc and the binary should agree before the next ops handoff — either the doc gets updated to the kill+up recipe, or the binary grows the alias.
