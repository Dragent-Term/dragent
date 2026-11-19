use crate::console::cmd_suggest::*;
fn test() {
    let s = ghost_suffix_from_cmd("git st");
    println!("{:?}", s);
}
