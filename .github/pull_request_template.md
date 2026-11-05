<!-- Pull Request Template

Adapted from "Writing good CL descriptions" in google/eng-practices,
licensed under CC BY 3.0. Modified for PRs.
https://github.com/google/eng-practices/blob/master/review/developer/cl-descriptions.md
-->

## What

<!--
1. Start with one imperative sentence stating the specific change.
2. Add only details needed to understand the change and its important limitations.
3. Prefer one small Mermaid diagram when the change involves interactions,
   shared dependencies, ownership, data flow, or state transitions. Include it
   even when concise prose could explain the same relationship.
   - Omit the diagram for an obvious standalone change such as a rename,
     constant, wording, styling, or simple display-order change. Do not draw
     a sentence as boxes, an unchanged generic pipeline, or authoring steps.
   - Choose by the question the reader needs answered: who communicates when
     (sequenceDiagram), which state changes under which condition
     (stateDiagram-v2), or what depends on or flows into what (flowchart).
     For races, retries, and cancellation, show the decisive ordering and
     actors. For lifecycle changes, show the states and transition conditions.
   - Draw the central mechanism of this change, not a secondary edge case.
     Keep its decisive inputs, conditions, and outcomes visible. For a shared
     rule, show what feeds it and who uses it; for separate paths, show where
     they diverge and what each changes. Omit unrelated implementation details.
     Keep What and Why understandable without rendering the diagram.

Keep prose under 120 words when practical. A simple change may need only one
sentence each in What and Why; do not pad it. Use short paragraphs or at most
five bullets per section, and do not repeat What in Why. Use at most one
focused diagram with only the actors or nodes needed to explain the change.
-->

## Why

<!-- Why are these changes being made? What context did you have as the
     author? Were there decisions you made that aren't reflected in the
     source code? -->

## Quality Assurance

<!-- Evidence the change works: manual verification steps, edge cases tested.
     - Do not mention checks guaranteed by CI (e.g. formatting, lint, build,
       automated tests). A speculative "CI is green" is sufficient.
     - For UI changes, show before and after with screenshots or videos. -->

## References

<!-- Links to related issues, design docs, and prior discussions. Note that
     external links may not be visible to future readers — include enough
     context in the description itself. -->
