---
name: crew-ssh
description: Run commands on, type into the user's terminal on, and copy files to or from, the user's saved SSH hosts (servers, VPSes, other machines) through Crew. Use whenever the user asks to check, manage, deploy to, debug or change a remote machine.
---

# Remote hosts through Crew

The user keeps their SSH hosts and credentials in Crew. Reach them only with the `crew-ssh` command: never ask for, read or handle passwords or keys, and don't fall back to `ssh`, `scp` or `sftp`.

- `crew-ssh hosts --json` lists the saved hosts: name, address, user, group, `notes` and `approval`.
- `crew-ssh exec <host> [--cwd DIR] "<command>"` runs a command on the host with its login shell and prints its output. It exits with the command's exit status, or 255 when Crew couldn't run it (the reason starts with `crew-ssh:`).
- `crew-ssh cp <local> <host>:<remote>` uploads one file; `crew-ssh cp <host>:<remote> <local>` downloads one.
- `crew-ssh screen <host> [--lines N] [--wait S]` prints what's on the user's terminal on that host in Crew (read it when they refer to something on their screen). `--wait` first waits up to S seconds for new output to come and go quiet.
- `crew-ssh send <host> [--keys K1,K2] [--wait S] ["<text>"]` types into that terminal, which the user shares with you and watches: the text, then the keys (`enter`, `tab`, `esc`, `ctrl+c`, `up`, `y`, …). It waits until the output is quiet for a second (at most S seconds, default 10) and prints what appeared. Example: `crew-ssh send web1 --keys enter "sudo systemctl restart nginx"`.

Put the command or text in one quoted argument after the options, with no `--` before it. Codex can't match its rules on a command line with a bare `--`, and then runs crew-ssh in its sandbox, where it can't reach Crew.

## exec or the shared terminal

Use `exec` for self-contained commands whose output you need; it doesn't touch the user's terminal. Use `send` when the work is interactive or belongs in the user's terminal: answering prompts, REPLs, editors, `top`, watching a long command, stopping one with `ctrl+c`, or continuing in the shell the user is in (its directory, environment, a `sudo -s` shell). `send` works only while the host's terminal is open in Crew. Read the screen first when you're unsure what's on it, and don't type over a line the user is writing. A quiet second doesn't mean a command finished: read more with `screen --wait`. When the terminal asks for a password, passphrase or code, Crew won't type it for you: ask the user to type it, then continue.

## Choosing a host

A host whose name matches the user's word exactly comes first; otherwise match addresses and groups. Ask only when one machine is meant and several match. For a group, work on each of its hosts. Each host's `notes` are the user's instructions for that machine: follow them.

If the machine isn't saved in Crew, say so and ask the user to add it (Hosts in Crew's sidebar).

## Approval

Commands that only read (`df -h`, `systemctl status nginx`, `journalctl -u app -n 200 --no-pager`, `docker ps` …), run or typed at the shell prompt, run at once. Anything that may change something, input into a full-screen program, and every upload, waits until the user approves it in Crew. Before such a command, say in one line what you'll run and why. Give `crew-ssh` a long timeout (10 minutes) so the user has time to approve, or to answer a password or host-key prompt on a first connection. If the user denies a command, don't run it again or work around it; ask them.

## Commands

`exec` has no terminal and no input: use non-interactive forms (`--no-pager`, `-y`, `DEBIAN_FRONTEND=noninteractive`, `systemctl --no-pager`). Keep output short (`| tail -n 100`). If `sudo` needs a password, ask the user to run that step. After a timeout or a `crew-ssh:` error, check what the command already did before running it again.

If `crew-ssh` isn't found or can't reach Crew, the session wasn't started from Crew or Crew isn't running: tell the user.
