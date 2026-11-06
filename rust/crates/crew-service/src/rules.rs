//! What agents may do without asking, and the keys they can press by name.

use std::collections::HashSet;
use std::sync::LazyLock;

use crew_term::Terminal;
use regex::Regex;

const READ_COMMANDS: &str = "ls ll cat head tail grep egrep fgrep rg stat file wc du df free uptime uname hostname whoami id groups who w last date pwd echo printf printenv which type ps pgrep vmstat iostat mpstat lscpu lsblk lsof lsmem lspci lsusb ss netstat ip ifconfig route ping traceroute tracepath dig nslookup host journalctl dmesg sort uniq cut tr tree realpath readlink basename dirname md5sum sha1sum sha256sum cksum getent nproc arch sensors zcat zgrep jq column nl diff cmp comm test true";

fn subcommand(name: &str) -> Option<&'static Regex> {
    static TABLE: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
        [
            ("systemctl", r"^(status|is-active|is-enabled|is-failed|list-units|list-unit-files|list-timers|show|cat)\b"),
            ("docker", r"^((ps|images|logs|inspect|version|info|top|port|stats\s.*--no-stream|history)\b|(container|image|network|volume)\s+(ls|list|inspect)\b|compose\s+(ps|logs|config|ls|images)\b)"),
            ("git", r"^(status|log|diff|show|branch|remote|rev-parse|describe|blame|ls-files|ls-remote|shortlog|reflog)\b"),
            ("kubectl", r"^(get|describe|logs|top|version|explain|api-resources|cluster-info|config\s+(view|get-contexts|current-context))\b"),
            ("apt", r"^(list|show|policy|search)\b"),
            ("apt-cache", r"^"),
            ("dpkg", r"^(-l|-L|-s|-S|--list|--status|--listfiles|--search)\b"),
            ("rpm", r"^-q"),
            ("yum", r"^(list|info|search|repolist)\b"),
            ("dnf", r"^(list|info|search|repolist)\b"),
            ("npm", r"^(ls|list|view|outdated|config\s+get)\b"),
            ("pip", r"^(list|show|freeze)\b"),
            ("pip3", r"^(list|show|freeze)\b"),
            ("pm2", r"^(list|ls|status|show|describe|logs\b.*--nostream)"),
            ("service", r"^\S+\s+status\b|^--status-all\b"),
            ("nginx", r"^(-t|-T|-v|-V)\s*$"),
            ("crontab", r"^-l\b"),
            ("ufw", r"^status\b"),
            ("timedatectl", r"^(status)?\s*$"),
            ("hostnamectl", r"^(status)?\s*$"),
            ("firewall-cmd", r"^(--state|--list-\S+|--get-\S+)(\s|$)"),
            ("iptables", r"^(-L|-S)\b"),
            ("find", r"^"),
            ("sed", r"^"),
            ("curl", r"^"),
            ("top", r"^-b\b"),
        ]
        .into_iter()
        .map(|(n, r)| (n, Regex::new(r).unwrap()))
        .collect()
    });
    TABLE.iter().find(|(n, _)| *n == name).map(|(_, r)| r)
}

/// Commands that run without approval in the default mode: every part of the line must be a read
/// (pipes and && / || / ; chains are split), with no redirection into files, no command substitution,
/// no sudo and no environment assignments. Anything unsure needs approval.
pub fn only_reads(command: &str) -> bool {
    static READS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| READ_COMMANDS.split(' ').collect());
    static SUBST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`|\$\(|<\(|>\(").unwrap());
    static HARMLESS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d?>>?\s*/dev/null|\d?>&\d").unwrap());
    static PARTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\|\|?|&&|;").unwrap());
    static GIT_OPTS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^((-C|-c)\s+\S+\s+|--no-pager\s+)+").unwrap());
    static FIND_WRITES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-(exec|execdir|ok|okdir|delete|fprint|fprintf|fls)\b").unwrap());
    static SED_WRITES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(^|\s)-[a-zA-Z]*i|--in-place").unwrap());
    static CURL_WRITES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(^|\s)(-[a-zA-Z]*[oOTdFX]|--(output|remote-name|upload-file|data\S*|form|request))").unwrap());
    let cmd = command.trim();
    if cmd.is_empty() || cmd.contains('\n') || SUBST.is_match(cmd) {
        return false;
    }
    // Redirections other than to /dev/null or between streams write files.
    if HARMLESS.replace_all(cmd, "").contains(['<', '>']) {
        return false;
    }
    for part in PARTS.split(cmd) {
        let mut words = part.split_whitespace();
        let Some(name) = words.next() else { return false };
        let args: Vec<&str> = words.collect();
        if name.contains('=') || ["sudo", "su", "xargs", "tee"].contains(&name) || (name == "env" && !args.is_empty()) {
            return false;
        }
        let mut rest = args.join(" ");
        if name == "git" {
            rest = GIT_OPTS.replace(&rest, "").into_owned();
        }
        if (name == "find" && FIND_WRITES.is_match(&rest)) || (name == "sed" && SED_WRITES.is_match(&rest)) || (name == "curl" && CURL_WRITES.is_match(&rest)) {
            return false;
        }
        if READS.contains(name) || subcommand(name).is_some_and(|r| r.is_match(&rest)) {
            continue;
        }
        return false;
    }
    true
}

/// Keys an agent can press, as a terminal sends them. Arrows, Home and End follow the terminal's
/// application cursor mode (set by full-screen programs).
pub fn key_sequence(name: &str, app_cursor: bool) -> Option<String> {
    let k = name.trim().to_lowercase();
    let csi = |c: char| if app_cursor { format!("\x1bO{c}") } else { format!("\x1b[{c}") };
    let named = match k.as_str() {
        "enter" | "return" => "\r".to_string(),
        "tab" => "\t".into(),
        "shift+tab" => "\x1b[Z".into(),
        "esc" | "escape" => "\x1b".into(),
        "backspace" => "\x7f".into(),
        "space" => " ".into(),
        "up" => csi('A'),
        "down" => csi('B'),
        "right" => csi('C'),
        "left" => csi('D'),
        "home" => csi('H'),
        "end" => csi('F'),
        "page_up" => "\x1b[5~".into(),
        "page_down" => "\x1b[6~".into(),
        "insert" => "\x1b[2~".into(),
        "delete" => "\x1b[3~".into(),
        "f1" => "\x1bOP".into(),
        "f2" => "\x1bOQ".into(),
        "f3" => "\x1bOR".into(),
        "f4" => "\x1bOS".into(),
        "f5" => "\x1b[15~".into(),
        "f6" => "\x1b[17~".into(),
        "f7" => "\x1b[18~".into(),
        "f8" => "\x1b[19~".into(),
        "f9" => "\x1b[20~".into(),
        "f10" => "\x1b[21~".into(),
        "f11" => "\x1b[23~".into(),
        "f12" => "\x1b[24~".into(),
        _ => String::new(),
    };
    if !named.is_empty() {
        return Some(named);
    }
    if let Some(c) = k.strip_prefix("ctrl+").filter(|c| c.len() == 1).and_then(|c| c.chars().next()).filter(char::is_ascii_lowercase) {
        return Some(((c as u8 - 96) as char).to_string());
    }
    let trimmed = name.trim();
    if let Some(rest) = trimmed.strip_prefix("alt+") {
        if rest.chars().count() == 1 {
            return Some(format!("\x1b{rest}"));
        }
    }
    (name.chars().count() == 1).then(|| name.to_string()) // one printable character
}

pub fn bad_key(keys: &[String]) -> Option<String> {
    let k = keys.iter().find(|k| key_sequence(k, false).is_none())?;
    Some(format!("Unknown key \"{k}\". Use one printable character or: enter, tab, shift+tab, esc, backspace, space, up, down, left, right, home, end, page_up, page_down, insert, delete, f1-f12, ctrl+a-ctrl+z, alt+<char>."))
}

const SUBMIT_KEYS: [&str; 5] = ["enter", "return", "ctrl+m", "ctrl+j", "ctrl+o"];

/// A prompt for a secret on the cursor's line: the user types those, never an agent.
pub fn asks_secret(cursor_line: &str) -> bool {
    static SECRET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(password|passphrase|passcode|密码|口令|verification code|otp)[^\n]*[:：]\s*$").unwrap());
    SECRET.is_match(cursor_line.trim())
}

/// How the input reads in an approval card and the activity log: the text, then the keys.
pub fn describe_input(text: &str, keys: &[String]) -> String {
    let text = text.replace("\r\n", "⏎").replace(['\r', '\n'], "⏎");
    let mut parts = Vec::new();
    if !text.is_empty() {
        parts.push(text);
    }
    parts.extend(keys.iter().map(|k| format!("[{k}]")));
    parts.join(" ")
}

/// Whether input runs without asking in the default mode: keys that don't submit anything at a shell
/// prompt, or a submitted line that only reads. Full-screen programs and everything else ask.
pub fn input_only_reads(term: &Terminal, text: &str, keys: &[String]) -> bool {
    line_only_reads(term.alt_screen(), &term.cursor_line(), text, keys)
}

fn line_only_reads(alt_screen: bool, cursor_line: &str, text: &str, keys: &[String]) -> bool {
    static PROMPT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^.*?[$#%>❯]\s").unwrap());
    let names: Vec<String> = keys.iter().map(|k| k.trim().to_lowercase()).collect();
    if alt_screen {
        return false;
    }
    let submits = |k: &String| SUBMIT_KEYS.contains(&k.as_str());
    if !text.contains(['\r', '\n']) && !names.iter().any(submits) {
        return !names.iter().any(|k| k == "ctrl+d"); // Ctrl+D can log out
    }
    if names.iter().any(|k| !submits(k)) {
        return false; // editing keys before Enter: ask
    }
    // The line that runs is what's already typed after the prompt plus the text. If the prompt can't
    // be told apart, the whole line is checked, which then asks.
    let typed = PROMPT.replace(cursor_line, "");
    let line = format!("{typed}{text}");
    let line = line.trim_end_matches(['\r', '\n']);
    !line.contains(['\r', '\n']) && only_reads(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_run_and_changes_ask() {
        for yes in ["df -h", "systemctl status nginx", "journalctl -u app -n 200 --no-pager", "docker ps", "cat /etc/os-release | grep NAME", "ls && df", "git -C /srv status", "ps aux 2>/dev/null", "find / -name x", "curl -s http://x", "sed -n 1,5p f", "uptime; free -m"] {
            assert!(only_reads(yes), "{yes}");
        }
        for no in ["rm -rf /", "sudo ls", "systemctl restart nginx", "echo x > f", "cat $(which ls)", "A=1 ls", "find / -delete", "sed -i s/a/b/ f", "curl -o f http://x", "ls\nrm x", "", "docker rm x", "git push", "ls | xargs rm", "env FOO=1 ls", "cat a | tee b"] {
            assert!(!only_reads(no), "{no}");
        }
    }

    #[test]
    fn keys_are_what_a_terminal_sends() {
        assert_eq!(key_sequence("enter", false).as_deref(), Some("\r"));
        assert_eq!(key_sequence("up", true).as_deref(), Some("\x1bOA"));
        assert_eq!(key_sequence("up", false).as_deref(), Some("\x1b[A"));
        assert_eq!(key_sequence("ctrl+c", false).as_deref(), Some("\x03"));
        assert_eq!(key_sequence("alt+x", false).as_deref(), Some("\x1bx"));
        assert_eq!(key_sequence("y", false).as_deref(), Some("y"));
        assert_eq!(key_sequence("nope", false), None);
        assert!(bad_key(&["enter".into(), "nope".into()]).unwrap().contains("nope"));
    }

    #[test]
    fn secrets_are_left_to_the_user() {
        assert!(asks_secret("[sudo] password for root: "));
        assert!(asks_secret("请输入密码："));
        assert!(!asks_secret("root@web:~# "));
    }

    #[test]
    fn typed_input_is_judged_by_the_line_it_runs() {
        let enter = vec!["enter".to_string()];
        assert!(line_only_reads(false, "root@web:~# ", "df -h", &enter));
        assert!(!line_only_reads(false, "root@web:~# ", "rm x", &enter));
        assert!(!line_only_reads(false, "root@web:~# rm ", "x", &enter), "what's already typed counts");
        assert!(!line_only_reads(true, "", "q", &[]), "full-screen programs ask");
        assert!(line_only_reads(false, "root@web:~# ", "partial", &[]), "nothing submitted");
        assert!(!line_only_reads(false, "root@web:~# ", "", &["ctrl+d".to_string()]));
        assert!(!line_only_reads(false, "root@web:~# ", "ls", &["tab".to_string(), "enter".to_string()]));
    }

    #[test]
    fn describes_input() {
        assert_eq!(describe_input("a\r\nb", &["enter".to_string()]), "a⏎b [enter]");
    }
}
