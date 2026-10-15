```
/create-skill 我现在希望创建一个skills用于review, AGENTS.md中描述了layout, 以及review需要参考的coding guidelines在仓库下的book/src/to-contribute/coding-guidelines

另外我希望这个skIll能着重检查, 是否有不必要的Arc使用, 以及冗余的.clone调用, 关于同步, 是否会发生锁使用的问题, 比如死锁, toctou, 以及代码风格检查下面的
Use block expressions when temporary variables are only needed to produce one final value. This keeps temporary state local and avoids leaking one-off names into outer scope.
Minimize nesting depth. Code nested more than three levels deep should be reviewed for refactoring opportunities. Each nesting level multiplies the reader’s cognitive load.

Techniques for flattening nesting:

Early returns and guard clauses for error paths.
let...else to collapse if let chains.
The ? operator for error propagation.
continue to skip loop iterations.
Extracting the nested body into a helper function.
The normal/expected code path should be the first visible path; error and edge cases should be handled and dismissed early.
```