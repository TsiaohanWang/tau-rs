// 入口文件。Cargo 需要一个 target（main.rs / lib.rs）才能加载这个 crate，
// 否则 rust-analyzer 会报 "Failed to load workspaces"。
// tau_agent 实现好之后，把下面这个 stub main 换成真正的逻辑即可。
mod tau_agent;

fn main() {
    println!("tau-rs");
}
