# Task spec column rows

`ts-rows.json` is the contract for `rh_tasks.spec`. The column is an object of per-task overrides. Identity (id, agent, goal, context, acceptance criteria, budget, session) lives in the other columns. The runtime spec is assembled from those columns plus the bag.

The first row is the live task whose spec object is only `excludeTools`. The other rows keep that task's columns and set one override key, a combination, an unknown key, or a value the typed bag rejects. Every row's node affinity is `a-node`: the recorded host token is on the privacy denylist, and that column is not part of the spec contract. `skipped` names the cases this slice does not replay, each with a reason. Tests compare the stored object as parsed JSON.
