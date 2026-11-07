//! What a row of the list says, and in what order (ADR 0064, `ui.md`).
//!
//! Must not depend on: `app` or winit.

use super::*;

/// A copy of `path` written at `seconds`, as a row of the list will show it.
fn plant_row(directory: &Path, index: usize, path: &str, seconds: u64) {
    let stem = format!("00000000000000a{index}-{seconds}-77-0");
    let meta = format!(
        "oso-emergency-copy 1\nname ignored\npath {path}\nwithheld false\nbytes 0\nwritten {seconds}\nwhy exit\n"
    );
    fs::write(directory.join(format!("{stem}.meta")), meta).unwrap();
    fs::write(directory.join(format!("{stem}.txt")), "").unwrap();
}

#[test]
fn a_row_starts_with_the_files_name_lined_up_and_ends_with_its_path() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    plant_row(&directory, 1, "/home/me/a.txt", 2);
    plant_row(&directory, 2, "/home/me/longer-name.txt", 1);
    let found = scan(&request(&directory, Vec::new()), later());
    let list = found.list.text().to_string();
    let rows: Vec<&str> = list.lines().skip(HEADER_ROWS).collect();
    assert!(rows[0].starts_with("a.txt   "), "{list}");
    assert!(rows[1].starts_with("longer-name.txt  "), "{list}");
    assert!(rows[0].ends_with("  /home/me/a.txt"), "{list}");
    assert!(rows[1].ends_with("  /home/me/longer-name.txt"), "{list}");
    let exit = |row: &str| row.find("exit").unwrap();
    assert_eq!(exit(rows[0]), exit(rows[1]), "the columns line up");
}

#[test]
fn a_wide_name_lines_up_by_its_cells() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    plant_row(&directory, 1, "/home/me/%E6%97%A5%E6%9C%AC.txt", 2);
    plant_row(&directory, 2, "/home/me/abcdefgh.txt", 1);
    let found = scan(&request(&directory, Vec::new()), later());
    let list = found.list.text().to_string();
    let rows: Vec<&str> = list.lines().skip(HEADER_ROWS).collect();
    assert!(rows[0].starts_with("日本.txt    "), "{list}");
    assert!(rows[1].starts_with("abcdefgh.txt  "), "{list}");
}

#[test]
fn a_row_takes_its_name_from_the_path_and_not_from_what_a_meta_calls_it() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    plant_row(&directory, 1, "/home/me/.bashrc", 1);
    let found = scan(&request(&directory, Vec::new()), later());
    let list = found.list.text().to_string();
    let row = list.lines().nth(HEADER_ROWS).unwrap();
    assert!(row.starts_with(".bashrc  "), "{list}");
    assert!(!row.contains("ignored"), "{list}");
}

#[test]
fn a_long_name_is_cut_and_the_path_still_holds_all_of_it() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let name = "n".repeat(60);
    plant_row(&directory, 1, &format!("/home/me/{name}"), 1);
    let found = scan(&request(&directory, Vec::new()), later());
    let list = found.list.text().to_string();
    let row = list.lines().nth(HEADER_ROWS).unwrap();
    assert!(row.starts_with(&format!("{}…  ", "n".repeat(31))), "{list}");
    assert!(row.ends_with(&format!("/home/me/{name}")), "{list}");
}

#[test]
fn a_copy_with_no_file_shows_its_name_once() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let meta = "oso-emergency-copy 1\nname *scratch*\nwithheld false\nbytes 0\nwhy exit\n";
    fs::write(directory.join(format!("{STEM}.meta")), meta).unwrap();
    fs::write(directory.join(format!("{STEM}.txt")), "").unwrap();
    let found = scan(&request(&directory, Vec::new()), later());
    let list = found.list.text().to_string();
    let row = list.lines().nth(HEADER_ROWS).unwrap();
    assert!(row.starts_with("*scratch*  "), "{list}");
    assert_eq!(row.matches("*scratch*").count(), 1, "{list}");
    assert!(row.ends_with("no file        "), "{list}");
}
