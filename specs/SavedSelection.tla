--------------------------- MODULE SavedSelection ---------------------------
\* session.ts, downstream of invocationRows (owner collisions and ordering
\* belong to HistoryRow). Two selected IDs in one workspace with core loaded;
\* a selection starts at the already projected row, including first-row claim.
\* Only the selected row is stored. "savedConsole" is in history but its
\* console wins; "live" is not the actual history record, even if non-console.
\* "saved" includes records with an attached non-console execution.
Kinds == {"detail", "profile"}
Rows == {"missing", "live", "diagnostic", "savedConsole", "saved"}
LogValues == {"pending", "incomplete", "settled"}
Empty == [k \in Kinds |-> "none"]
Phases(p) == [k \in Kinds |-> p]
VARIABLES id, row, onPage, phase, cache, retired, historyTick, wrapper
vars == <<id, row, onPage, phase, cache, retired, historyTick, wrapper>>
Eligible == onPage /\ row = "saved"
Loading == {k \in Kinds : phase[k] = "loading"}
Init == /\ id = "A" /\ row = "missing" /\ onPage = TRUE
        /\ phase = Phases("empty") /\ cache = Empty /\ retired = {}
        /\ historyTick = FALSE /\ wrapper = FALSE

\* retired coalesces all orphaned requests of each kind. One arbitrary
\* late success/rejection is representative: neither can publish through
\* the new wrapper. The toggle observes replacement, NOT identity equality
\* across time (A -> B -> A never revives a retired job).
Replace(values, start) ==
    /\ wrapper' = ~wrapper /\ retired' = retired \cup Loading
    /\ cache' = values
    /\ phase' = [k \in Kinds |-> IF start THEN "loading"
                                 ELSE IF values[k] = "none" THEN "empty" ELSE "ready"]
Ensure ==
    /\ phase' = [k \in Kinds |-> IF phase[k] = "empty" THEN "loading" ELSE phase[k]]
    /\ UNCHANGED <<cache, retired, wrapper>>
Select(r) ==
    /\ id' = IF id = "A" THEN "B" ELSE "A"
    /\ onPage' = TRUE /\ row' = r /\ Replace(Empty, r = "saved")
    /\ UNCHANGED historyTick
Navigate ==
    /\ onPage' = ~onPage /\ UNCHANGED <<id, row, historyTick>>
    /\ IF onPage' /\ row = "saved" THEN Ensure
       ELSE UNCHANGED <<cache, phase, retired, wrapper>>

\* Push models an eligibility change without a changed selected history
\* record (e.g. a console attaches or a diagnostic wins). A successful list
\* reply additionally observes JSON record change or a ready detail/record
\* mismatch. Those comparisons are abstract input predicates, not clocks.
\* A store error that only updates diagnostics takes the Push path. List
\* queuing, pin retries and workspace disposal are outside this instance.
AwaitingEvents == cache["detail"] \in {"pending", "incomplete"} \/
                  (phase["detail"] = "loading" /\ cache["detail"] = "none")
Update(r, reason) ==
    /\ row' = r /\ UNCHANGED <<id, onPage>>
    /\ historyTick' = IF reason # "push" THEN ~historyTick ELSE historyTick
    /\ (reason = "absent" => r \notin {"saved", "savedConsole"})
    /\ (reason = "mismatch" => phase["detail"] = "ready")
    /\ LET eligible == onPage /\ r = "saved"
           refresh == reason \in {"changed", "mismatch"} \/
                      (reason = "unchangedPresent" /\ phase["detail"] # "failed" /\ AwaitingEvents)
       IN IF Eligible /\ ~eligible THEN Replace(Empty, FALSE)
          ELSE IF refresh THEN Replace(IF onPage THEN cache ELSE Empty, eligible)
          ELSE IF ~Eligible /\ eligible THEN Ensure
          ELSE UNCHANGED <<cache, phase, retired, wrapper>>

\* Manual per-tab retry/reload. Re-selecting the same ID retries only the
\* failed slots; it does not replace the wrapper. Returning to History
\* starts empty slots, not failed ones.
Read(k) ==
    /\ Eligible /\ k \notin Loading
    /\ phase' = [phase EXCEPT ![k] = "loading"]
    /\ UNCHANGED <<cache, retired, id, row, onPage, wrapper, historyTick>>
Reselect ==
    /\ Eligible
    /\ phase' = [k \in Kinds |-> IF phase[k] \in {"failed", "empty"}
                                 THEN "loading" ELSE phase[k]]
    /\ UNCHANGED <<cache, retired, id, row, onPage, wrapper, historyTick>>

Reply(k,v) ==
    /\ k \in Loading
    /\ phase' = [phase EXCEPT ![k] =
         IF ~Eligible THEN (IF cache[k] = "none" THEN "empty" ELSE "ready")
         ELSE IF v = "error" THEN "failed" ELSE "ready"]
    /\ cache' = IF Eligible /\ v # "error" THEN [cache EXCEPT ![k] = v] ELSE cache
    /\ UNCHANGED <<id, row, onPage, retired, wrapper, historyTick>>
Late(k) ==
    /\ k \in retired /\ retired' = retired \ {k}
    /\ UNCHANGED <<id, row, onPage, cache, phase, wrapper, historyTick>>
Next == Navigate \/ Reselect \/ (\E r \in Rows : Select(r)) \/
        (\E r \in Rows, why \in {"push","changed","mismatch","unchangedPresent","absent"} :
            Update(r,why)) \/
        (\E k \in Kinds : Read(k) \/ Late(k) \/
          (\E v \in (IF k = "detail" THEN LogValues ELSE {"profile"}) \cup {"error"} :
            Reply(k,v)))
Spec == Init /\ [][Next]_vars

CacheShape == \A k \in Kinds : (phase[k] = "ready" => cache[k] # "none") /\
                               (phase[k] = "empty" => cache[k] = "none")
LateSilent == [][retired \ retired' # {} => UNCHANGED <<cache, phase>>]_vars
FreshSelection == [][id # id' => cache' = Empty /\
                     phase' = Phases(IF row' = "saved" THEN "loading" ELSE "empty")]_vars
LoseEligibility == [][Eligible /\ onPage' /\ row' # "saved" =>
                      cache' = Empty /\ phase' = Phases("empty") /\ wrapper' # wrapper]_vars
RetainedRefresh == [][id' = id /\ Eligible /\ row' = "saved" /\ wrapper' # wrapper =>
                      cache' = cache /\ phase' = Phases("loading")]_vars
RetireBoth == [][wrapper' # wrapper => Loading \subseteq retired']_vars
StartOnlyEligible == [][\A k \in Kinds : phase[k] # "loading" /\ phase'[k] = "loading" =>
                        onPage' /\ row' = "saved"]_vars
FailureKeepsValue == [][\A k \in Loading : phase'[k] = "failed" =>
                        cache'[k] = cache[k]]_vars
HistoryDuringReceipt ==
    [][historyTick' # historyTick /\ Eligible /\ row' = "saved" /\
       phase["detail"] # "failed" /\ AwaitingEvents => wrapper' # wrapper]_vars
NewValueRequiresRead ==
    [][\A k \in Kinds : cache'[k] # "none" /\ cache'[k] # cache[k] =>
       Eligible /\ k \in Loading /\ phase'[k] = "ready" /\
       \A other \in Kinds \ {k} : cache'[other] = cache[other] /\ phase'[other] = phase[other]]_vars
=============================================================================
