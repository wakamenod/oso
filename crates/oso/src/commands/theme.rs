//! Commands that choose the theme the frame is drawn in (ADR 0089).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};

/// The commands for themes. Both are the person's: the theme is theirs, and it is
/// remembered on disk.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "load-theme",
        doc: "Draw with the theme named in a prompt, which completes over the themes.\n\nThe choice is remembered for the next start.",
        args: ArgSpec::Theme {
            prompt: "Load theme: ",
            history: "theme",
        },
        reach: Reach::Person,
        run: load_theme,
    },
    Command {
        name: "toggle-theme",
        doc: "Switch between the dark and the light theme.\n\nFrom another theme, switch to the dark one. The choice is remembered for the next start.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: toggle_theme,
    },
];

fn load_theme(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let Some(name) = ctx.answer() else {
        return Err(CommandError::new("load-theme needs a theme's name"));
    };
    ctx.editor
        .load_theme(name)
        .map_err(|error| CommandError::new(error.to_string()))?;
    ctx.request_redraw();
    Ok(())
}

fn toggle_theme(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if !ctx.editor.toggle_theme() {
        return Err(CommandError::new("No theme to switch to"));
    }
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::commands::tests::{editor, run};
    use oso_core::command::{Args, Ctx, Invocation};
    use oso_core::editor::Editor;

    fn themed() -> Editor {
        let mut editor = editor("abc");
        editor.set_themes(
            vec![("dark", "dark"), ("light", "light"), ("dusk", "dark")],
            0,
        );
        editor
    }

    fn load(editor: &mut Editor, answer: &str) -> (Result<(), String>, bool) {
        let registry = crate::commands::registry();
        let id = registry.lookup("load-theme").unwrap();
        let invocation = Invocation {
            answer: Some(answer),
            ..Invocation::default()
        };
        let mut ctx = Ctx::invoked(editor, invocation);
        let result = registry.execute(id, &mut ctx, Args::NONE);
        let redraw = ctx.effects().redraw;
        (result.map_err(|error| error.to_string()), redraw)
    }

    #[test]
    fn load_theme_draws_with_the_theme_named_and_says_so() {
        let mut editor = themed();
        assert_eq!(load(&mut editor, "dusk"), (Ok(()), true));
        assert_eq!(editor.theme_name(), Some("dusk"));
        let said = crate::commands::messages::kept_messages(&editor);
        assert_eq!(said.last().map(String::as_str), Some("Theme: dusk"));
    }

    #[test]
    fn an_unknown_theme_changes_nothing_and_draws_nothing() {
        let mut editor = themed();
        let (result, redraw) = load(&mut editor, "sepia");
        assert!(result.unwrap_err().contains("No theme named sepia"));
        assert!(!redraw);
        assert_eq!(editor.theme_name(), Some("dark"));
    }

    #[test]
    fn toggle_theme_switches_the_pair_and_draws() {
        let mut editor = themed();
        let (result, effects) = run(&mut editor, "toggle-theme", Args::NONE);
        assert_eq!(result, Ok(()));
        assert!(effects.redraw);
        assert_eq!(editor.theme_name(), Some("light"));
        let _ = run(&mut editor, "toggle-theme", Args::NONE);
        assert_eq!(editor.theme_name(), Some("dark"));
    }

    #[test]
    fn with_no_themes_the_toggle_says_so() {
        let mut editor = editor("abc");
        let (result, effects) = run(&mut editor, "toggle-theme", Args::NONE);
        assert_eq!(result, Err("No theme to switch to".to_owned()));
        assert!(!effects.redraw);
    }
}
