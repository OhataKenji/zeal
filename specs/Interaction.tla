---------------------------- MODULE Interaction ----------------------------
\* A sum of independent component instances, NOT a shared application lock.
\* text stands for trimmed path / comparison ID / scope, with two different
\* nonempty values. gate is Bazel AND nonblank patterns for a prompt, or a
\* settled, non-running, error-free left side for a comparison. running is
\* right-ID membership, not a permanent ban on receiving its reply.
\* last records the result of the last continuation, not UI state.
Text == {"blank", "A", "B"}
VARIABLES kind, phase, mounted, pending, text, gate, running, opening,
          canClose, closeTick, last
vars == <<kind, phase, mounted, pending, text, gate, running, opening,
          canClose, closeTick, last>>
Init == /\ kind \in {"prompt", "comparison", "scope"}
        /\ phase = "idle" /\ mounted = TRUE /\ pending = "none" /\ text \in Text
        /\ gate \in BOOLEAN /\ running \in BOOLEAN /\ opening = FALSE
        /\ canClose \in BOOLEAN /\ closeTick = FALSE /\ last = "none"
Busy == phase \in {"choosing", "submitting", "loading", "saving"} \/
        (kind = "prompt" /\ opening)

\* Form edits are blocked while busy. WorkspaceView can independently
\* reseed the draft on a changed persistedScope key, even during save.
Edit == /\ mounted /\ (~Busy \/ kind = "scope")
        /\ (kind = "comparison" => gate)
        /\ text' \in Text
        /\ UNCHANGED <<kind, phase, mounted, pending, gate, running, opening,
                       canClose, closeTick, last>>
Props == /\ mounted /\ gate' \in BOOLEAN /\ running' \in BOOLEAN /\ canClose' \in BOOLEAN
         /\ IF kind = "prompt" /\ phase # "submitting" THEN opening' \in BOOLEAN
            ELSE opening' = opening
         /\ UNCHANGED <<kind, phase, mounted, pending, text, closeTick, last>>

Begin(p) == /\ mounted /\ ~Busy
            /\ phase' = p /\ pending' = p /\ last' = "none"
            /\ opening' = (opening \/ p = "submitting")
            /\ UNCHANGED <<kind, mounted, text, gate, running, canClose, closeTick>>
Pick == kind = "prompt" /\ Begin("choosing")
Submit == kind = "prompt" /\ gate /\ text # "blank" /\ Begin("submitting")
Save == kind = "scope" /\ text # "blank" /\ Begin("saving")
Compare ==
    /\ kind = "comparison" /\ mounted /\ ~Busy /\ gate /\ text # "blank"
    /\ IF running THEN /\ phase' = "failed" /\ last' = "running"
                       /\ UNCHANGED <<kind, mounted, pending, text, gate, running,
                                      opening, canClose, closeTick>>
       ELSE Begin("loading")

Cancel == /\ kind = "prompt" /\ mounted /\ ~Busy /\ canClose
          /\ closeTick' = ~closeTick /\ last' = "cancel"
          /\ UNCHANGED <<kind, phase, mounted, pending, text, gate, running, opening, canClose>>

\* Unmount is external, distinct from calling the optional onclose callback.
\* Only ComparisonPanel retires its request on destruction. Picker/submit
\* and save continuations can run after unmount. The parent/session's own
\* openWorkspace acceptance, UI teardown and error presentation are separate.
Unmount == /\ mounted /\ mounted' = FALSE
           /\ phase' = IF kind = "comparison" THEN "disposed" ELSE phase
           /\ UNCHANGED <<kind, pending, text, gate, running, opening, canClose, closeTick, last>>
Outcomes ==
    CASE pending = "choosing" -> {"A", "B", "null", "error"}
      [] pending \in {"saving", "submitting"} -> {"true", "false", "error"}
      [] pending = "loading" -> {"detail", "error"}
      [] OTHER -> {}
Finish(result) ==
    /\ result \in Outcomes /\ pending' = "none" /\ last' = result
    /\ phase' = CASE phase = "disposed" -> "disposed"
                  [] result = "error" -> "failed"
                  [] result = "detail" -> "ready"
                  [] OTHER -> "idle"
    /\ text' = IF pending = "choosing" /\ result \in {"A","B"} THEN result ELSE text
    /\ opening' = IF pending = "submitting" THEN FALSE ELSE opening
    /\ closeTick' = IF pending = "submitting" /\ result = "true" /\ canClose
                    THEN ~closeTick ELSE closeTick
    /\ UNCHANGED <<kind, mounted, gate, running, canClose>>
Next == Edit \/ Props \/ Pick \/ Submit \/ Save \/ Compare \/ Cancel \/ Unmount \/
        (\E v \in Outcomes : Finish(v)) \/ (~mounted /\ pending = "none" /\ UNCHANGED vars)
Spec == Init /\ [][Next]_vars

OwnedWait == IF phase = "disposed" THEN ~mounted /\ kind = "comparison" /\
                                    pending \in {"none", "loading"}
             ELSE IF pending # "none" THEN phase = pending
             ELSE phase \in {"idle", "ready", "failed"}
Admission ==
    [][pending = "none" /\ pending' # "none" =>
       mounted /\ ~Busy /\
       CASE pending' = "choosing" -> kind = "prompt"
         [] pending' = "submitting" -> kind = "prompt" /\ gate /\ text # "blank" /\ opening'
         [] pending' = "loading" -> kind = "comparison" /\ gate /\ ~running /\ text # "blank"
         [] OTHER -> kind = "scope" /\ text # "blank"]_vars
PromptCompletion ==
    [][pending = "submitting" /\ pending' = "none" =>
       ~opening' /\ ((closeTick' # closeTick) = (last' = "true" /\ canClose)) /\
       phase' = (IF last' = "error" THEN "failed" ELSE "idle")]_vars
PickerCancellation ==
    [][pending = "choosing" /\ pending' = "none" /\ last' = "null" =>
       phase' = "idle" /\ UNCHANGED <<text, closeTick>>]_vars
CancelOnlyIdle == [][closeTick' # closeTick /\ last' = "cancel" =>
                    mounted /\ ~Busy /\ canClose /\ kind = "prompt"]_vars
ComparisonOwnership ==
    [][kind = "comparison" =>
       (phase = "disposed" => phase' = "disposed") /\
       (phase # "ready" /\ phase' = "ready" =>
        mounted /\ phase = "loading" /\ pending = "loading" /\ last' = "detail")]_vars
SaveLocalResult ==
    [][pending = "saving" /\ pending' = "none" =>
       phase' = (IF last' = "error" THEN "failed" ELSE "idle") /\ UNCHANGED text]_vars
=============================================================================
