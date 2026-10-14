# agent-loop safety fixtures (B15)

Written for RivetHub, not recorded from the TypeScript. Each row is a tool call and the outcome the safety hooks
must reach. `fence.json` is the workspace fence: the allowed directories, the tool call, and whether it is `allow` or
`block`. `shell.json` is the shell danger check: a command and whether it is `allow` or `block`. The rules fail closed.
Path containment is component-wise on normalised paths, so a shared-prefix sibling, `..` and a symlink out of the
workspace are refused. Arguments of the wrong type are refused. A home directory is never guessed.
