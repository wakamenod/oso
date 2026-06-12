;;; .dir-locals.el --- Tokimori project-local Emacs settings -*- mode: lisp -*-

;; ── 全バッファ共通 ────────────────────────────────────────────────────────────
((nil
  ;; Projectile per-project just コマンド
  ;;   C-c p c  → projectile-compile-project  → just build
  ;;   C-c p P  → projectile-test-project     → just test
  ;;   C-c p u  → projectile-run-project      → just open (Xcode を開く)
  (projectile-project-compilation-dir . ".")
  (projectile-project-compilation-cmd . "just build")
  (projectile-project-test-cmd        . "just test")
  (projectile-project-run-cmd         . "just run")))
