;;; .dir-locals.el --- oso project-local Emacs settings -*- mode: lisp -*-

;; ── All buffers ──────────────────────────────────────────────────────────────
((nil
  ;; Projectile per-project just commands
  ;;   C-c p c  → projectile-compile-project  → just build
  ;;   C-c p P  → projectile-test-project     → just test
  ;;   C-c p u  → projectile-run-project      → just run
  (projectile-project-compilation-dir . ".")
  (projectile-project-compilation-cmd . "just build")
  (projectile-project-test-cmd        . "just test")
  (projectile-project-run-cmd         . "just run")))
