---------------------------- MODULE AsyncRequest ----------------------------
\* Explicit state model for `src/lib/request.ts` (`createRequest`), the shared
\* owned async request controller used by `createAnalytics` and
\* `ComparisonPanel`.
\*
\* Models finite bounded request tokens `1..MaxReq` with optional value
\* retention during reload (`retainValue`), invalidation, explicit failure,
\* stale-reply suppression, and terminal disposal.
EXTENDS Naturals

CONSTANTS MaxReq

ASSUME MaxReq \in Nat /\ MaxReq >= 1

None == 0
Req == 1..MaxReq
Kinds == {"idle", "loading", "ready", "failed", "disposed"}

VARIABLES
  kind,
  owner,
  pending,
  nextId,
  value,
  hasError,
  pub

vars == <<kind, owner, pending, nextId, value, hasError, pub>>

TypeOK ==
  /\ kind \in Kinds
  /\ owner \in Req \cup {None}
  /\ pending \subseteq Req
  /\ nextId \in 1..(MaxReq + 1)
  /\ value \in Req \cup {None}
  /\ hasError \in BOOLEAN
  /\ pub \in BOOLEAN

Init ==
  /\ kind = "idle"
  /\ owner = None
  /\ pending = {}
  /\ nextId = 1
  /\ value = None
  /\ hasError = FALSE
  /\ pub = FALSE

Run(retain) ==
  /\ kind # "disposed"
  /\ nextId <= MaxReq
  /\ LET r == nextId IN
     /\ kind' = "loading"
     /\ owner' = r
     /\ pending' = pending \cup {r}
     /\ nextId' = nextId + 1
     /\ value' = IF retain THEN value ELSE None
     /\ hasError' = FALSE
     /\ pub' = ~pub

ReplyOk(r) ==
  /\ r \in pending
  /\ pending' = pending \ {r}
  /\ IF owner = r
     THEN /\ kind' = "ready"
          /\ owner' = None
          /\ value' = r
          /\ hasError' = FALSE
          /\ pub' = ~pub
     ELSE UNCHANGED <<kind, owner, value, hasError, pub>>
  /\ UNCHANGED nextId

ReplyErr(r) ==
  /\ r \in pending
  /\ pending' = pending \ {r}
  /\ IF owner = r
     THEN /\ kind' = "failed"
          /\ owner' = None
          /\ value' = None
          /\ hasError' = TRUE
          /\ pub' = ~pub
     ELSE UNCHANGED <<kind, owner, value, hasError, pub>>
  /\ UNCHANGED nextId

Invalidate ==
  /\ kind # "disposed"
  /\ kind' = "idle"
  /\ owner' = None
  /\ value' = None
  /\ hasError' = FALSE
  /\ pub' = ~pub
  /\ UNCHANGED <<pending, nextId>>

Fail ==
  /\ kind # "disposed"
  /\ kind' = "failed"
  /\ owner' = None
  /\ value' = None
  /\ hasError' = TRUE
  /\ pub' = ~pub
  /\ UNCHANGED <<pending, nextId>>

Dispose ==
  /\ kind # "disposed"
  /\ kind' = "disposed"
  /\ owner' = None
  /\ UNCHANGED <<pending, nextId, value, hasError, pub>>

DrainReply ==
  \E r \in pending : ReplyOk(r) \/ ReplyErr(r)

Quiet ==
  /\ pending = {}
  /\ kind = "disposed" \/ nextId > MaxReq
  /\ UNCHANGED vars

Next ==
  \/ Run(FALSE)
  \/ Run(TRUE)
  \/ DrainReply
  \/ Invalidate
  \/ Fail
  \/ Dispose
  \/ Quiet

Spec ==
  Init /\ [][Next]_vars /\ WF_vars(DrainReply)

\* Safety invariants
LoadingHasOwner == (kind = "loading") <=> (owner # None)
OwnerIsPending == owner # None => owner \in pending
ReadyHasValue == kind = "ready" => (value # None /\ ~hasError)
FailedHasNoValue == kind = "failed" => (value = None /\ hasError)
IdleIsClean == kind = "idle" => (value = None /\ ~hasError)
DisposedHasNoOwner == kind = "disposed" => owner = None
ValueFromPastRequest == value # None => value < nextId

\* Action and liveness properties
DisposedSilent ==
  [][kind = "disposed" => UNCHANGED <<kind, owner, value, hasError, pub>>]_vars

NoStaleTakeover ==
  [][\A r \in pending :
      (r # owner /\ r \notin pending') => UNCHANGED <<kind, owner, value, hasError, pub>>]_vars

EventualDrain == <>(pending = {})
=============================================================================
