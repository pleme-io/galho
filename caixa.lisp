;; galho — branch-aware typed IaC state (caixa Biblioteca kind).
;;
;; Substrate-promoted form of the galho pattern documented in
;; pleme-io/theory/GALHO.md. Solves the structural conflict between git's
;; immutable-history-with-deferred-merge and IaC's shared-mutable-state-with-
;; immediate-apply (branch-clobber, plan-staleness, apply-before-vs-after-merge,
;; review fatigue) by lifting state into a typed, content-addressed Merkle DAG.
;;
;; IaC-system-agnostic algebra: canonical Resource Graph IR + IaCSystem trait;
;; terraform first-class via galho-magma (M3); crossplane / helm / pulumi /
;; ansible / nix follow. Content-addressed Merkle DAG with three-way merge
;; algebra over the IR; typed Plan<S> value (not text); typed Conflict CR for
;; structural / semantic / cross-system conflicts; multi-IaC galhos with
;; auto-DAG promotion from typed cross-system references.
;;
;; Composes with carve: carve-driven mode creates Galhos alongside the stacked
;; PRs carve opens; `galho gate` is sibling of `carve gate`; vitrine renders
;; Plan<S> into PR bodies as pre-merge evidence; carve's "Done" jira-sync is
;; gated on galho Settled phase — "Done" stops being "merged" and becomes
;; "merged AND deployed."
;;
;; Crates (v0.1 — M1):
;;   galho-types     — canonical Resource Graph IR + CanonicalBytes (consumes
;;                     tameshi::Canonicalizer) + TypedState<S> + Plan<S> +
;;                     TypedConflict + three-way merge algebra. 24 tests.
;;   galho-storage   — git-style content-addressed object store + Merkle DAG +
;;                     reference-counted GC. Backends: MemoryBackend (tests),
;;                     LocalFsBackend (canonical dev). 12 tests.
;;
;; Crates (M2+):
;;   galho-terraform — reference IaCSystem adapter via magma's StateBackend.
;;   galho-helm      — Helm release state adapter.
;;   galho-crossplane — managed-resource CR adapter.
;;   galho-controller — engenho_controllers::Controller impl (canonical second
;;                      consumer; webhook ingress; 10-phase state machine).
;;   galho-cli       — `galho new / status / plan / apply / promote / rebase
;;                      / merge / diff / list / destroy / import / resolve /
;;                      drift / gate` operator CLI.
;;
;; Consumes — does not extract — three load-bearing pleme-io traits, advancing
;; each toward fleet-wide saturation per the org Prime Directive three-site rule:
;;   tameshi::canonicalize::Canonicalizer   (canonical second consumer)
;;   engenho_controllers::Controller         (canonical second consumer)
;;   shigoto::Dag                             (per-transition typed job graph)
;;
;; Two extractions deferred per three-site rule, with named triggers in the
;; canonical spec:
;;   - Merkle-DAG / content-addressed object store primitive: re-evaluate when
;;     carve's backup-tag chain or shigoto's audit-log emitter develops the
;;     same shape.
;;   - Typed three-way-merge primitive: re-evaluate when tatara-lisp, typescape,
;;     or caixa develop AST-merge needs (dq-merge is the untyped reference).
;;
;; Build:
;;   nix build .#galho-types
;;   nix build .#galho-storage
;;   cargo test --workspace      ; 36 tests pass
;;
;; Spec:
;;   pleme-io/theory/GALHO.md     ; canonical destination spec (~2000 lines)
;;   pleme-io/docs/galho.md       ; operator-facing primer

(defcaixa
  :nome        "galho"
  :versao      "0.1.0"
  :kind        Biblioteca
  :edicao      "2026"
  :descricao   "Branch-aware typed IaC state — every PR touching infrastructure gets an isolated typed state-projection that inherits from its base. Canonical Resource Graph IR + IaCSystem trait (terraform first-class via magma; crossplane / helm / pulumi / ansible / nix follow). Content-addressed Merkle DAG with three-way merge algebra; typed Plan<S> value (not text); typed Conflict CR; multi-IaC galhos with auto-DAG promotion. Composes with carve (PR shape) + vitrine (PR evidence) + saguão (authz) + cofre (secrets) + viggy / promessa (continuous outcomes) + tameshi (audit chain)."
  :repositorio "github:pleme-io/galho"
  :licenca     "MIT"
  :autores     ("pleme-io")
  :etiquetas   ("galho" "iac" "terraform" "crossplane" "helm" "branch-state" "merkle-dag" "three-way-merge" "content-addressed" "caixa-biblioteca")
  :deps        ("tameshi" "engenho-controllers")
  :deps-dev    ())
