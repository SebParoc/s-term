# Short prompt inside S-term: the pane header already shows user@host:path (from the window title),
# so the prompt is just an arrow, red when the last command failed. Other terminals keep the normal prompt.
# Settings → Terminal → "Before the ❯" adds the working folder (~/s-term ❯) or user@host and the folder (user@host:~/s-term ❯).
# S-term signals them with flag files, checked at every prompt, so the switches reach panes that are already open.
if [ "$TERM_PROGRAM" = "s-term" ] && [[ $- == *i* ]]; then
    __s_term_prompt() {
        local s=$? pre='' cfg="${XDG_CONFIG_HOME:-$HOME/.config}/s-term"
        if [ -e "$cfg/prompt-user" ]; then
            pre='\[\e[1;32m\]\u@\h\[\e[0m\]'
            [ -e "$cfg/prompt-dir" ] && pre+=':' || pre+=' '
        fi
        if [ -e "$cfg/prompt-dir" ]; then
            pre+='\[\e[1;34m\]\w\[\e[0m\] '
        fi
        PS1="$pre\[\e[1;$(( s ? 31 : 35 ))m\]❯\[\e[0m\] "
        return $s
    }
    PROMPT_COMMAND="__s_term_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
fi
