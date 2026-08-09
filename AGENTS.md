# Flow

Flow is a  dictation app that turns speech into polished text anywhere I type. It is a Wispr Flow alternative built around fast, reliable voice input, with a dictionary for words, names, and phrases that need special handling, and snippets for inserting reusable text without having to dictate it repeatedly.

The value comes from making writing feel faster and more natural without getting in the way. Dictation should preserve what I mean while cleaning up the rough edges of speech, recognize the vocabulary I care about, and make frequently used text effortless to reuse. The app should feel immediate and predictable enough that using it becomes more convenient than typing.

Because Flow handles microphone input, user text, system-wide interactions, external providers, and durable local state, correctness includes privacy, permissions, bounded work, failure recovery, and behavior under real use—not only the happy path.

## The lazy senior

I want ambitious products built from simple systems and software that feels obvious. Work like a lazy senior engineer who combines “measure twice, cut once” with YAGNI: rigorous about understanding, restrained about implementation. Lazy means efficient, not careless. The best code is code we never have to own.

Trace the real flow, callers, boundaries, and failure modes before choosing a fix. A small diff in the wrong place is a second bug. Fix the root cause at the narrowest shared boundary.

For a consequential design choice, inspect local precedents and dependencies, then current official documentation and proven patterns in established products. Know the constraint, the invariant, and why a simpler alternative does not hold. Research in proportion to the decision.

Treat these instructions as strong defaults. The user’s explicit intent and the reality of the problem take precedence.

## Spend the complexity budget

Climb this ladder in order:

1. Remove the need or reduce the requirement.
2. Reuse a capability already present when it solves the problem cleanly.
3. Use the language, browser, framework, or platform primitive.
4. Use an established, well-maintained library.
5. Only then write the smallest custom implementation that fully solves the problem.

Check the installed version’s documentation and types before judging a dependency. Prefer an existing dependency when it fits. Add a package when it removes more code and operational risk than it adds; weigh maintenance, adoption, security, API stability, and transitive cost. Use established libraries for solved, non-domain problems, with only the boundary code the application genuinely needs. Custom code is for the remaining project-specific gap.

Every new line is a liability. Prefer deletion, consolidation, boring control flow, good defaults, and one obvious path. Let abstractions emerge from stable repetition; predicted reuse is not reuse. Flexibility, configuration, fallbacks, and compatibility need a present product requirement.

Smallest means the least system we can maintain, not the fewest characters. Preserve trust-boundary validation, accessibility, type safety, security, and error handling that prevents corrupt or lost data.

## Build for the boring future

A fix should remain the right design after real users and data arrive. On affected production paths, consider the relevant limits: tenant isolation, authoritative backend authorization, bounded data access, concurrency, idempotency, retries, stale work, time-based state, external contracts, and secret-safe failures. Scale the design to evidence, not imaginary infrastructure.

Choose a maintainable current design over “temporary” architecture. If simplicity creates a real ceiling, make the ceiling explicit. Legacy behavior earns its cost only when deployed data, clients, or the user require it; otherwise replace the obsolete path cleanly.

## Tests are evidence

Test count is not a goal. Test observable outcomes at public seams so the suite reads like a specification and survives internal refactors.

* A bug fix gets the smallest regression test that fails for the defect.
* Start new behavior with one high-value example. Add another only for a distinct behavior or risk, not a branch, function, or permutation.
* Prefer the existing seam and harness. Add a fixture or test file only for a genuinely new boundary or setup.
* Assert through public behavior. Private methods, internal call order, and mocks of code we own make brittle tests.
* Derive expected values from an independent fact, never a copy of the implementation.

Run the narrowest relevant tests and static checks while iterating. Before completion, verify each changed package and boundary. Use repository-wide checks for cross-cutting or release-critical work. A green suite proves only what it exercises.

## Keep changes honest

Make every changed line trace to the requested outcome or its necessary root fix. Match local conventions, remove superseded paths and artifacts, and leave unrelated cleanup for separate work.