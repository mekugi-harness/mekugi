export LC_ALL=C GIT_NO_LAZY_FETCH=1 GIT_OPTIONAL_LOCKS=0 GIT_TERMINAL_PROMPT=0
capture() {
    local output
    output=$({ "$@" 2>&1; printf '\n__exit=%s' "$?"; } | dd bs=1 count=16385 2>/dev/null)
    if ((${#output} >= 16385)); then
        printf '%s\n[truncated]' "${output:0:16384}"
        return 0
    fi
    printf '%s' "${output%$'\n'__exit=*}"
    return "${output##*$'\n'__exit=}"
}
git_read() {
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_COUNT=0 GIT_CONFIG_PARAMETERS= git --no-lazy-fetch --no-optional-locks -c core.fsmonitor=false -c log.showSignature=false "$@"
}
printf 'Observed at: %s\nPlatform: %s\nHome: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$0" "${HOME:-unavailable}"
if [[ $0 == linux ]]; then
    os=$(capture sed -n 's/^PRETTY_NAME=//p' /etc/os-release) || os=$(capture sed -n 's/^PRETTY_NAME=//p' /usr/lib/os-release) || os=unavailable
    os=${os#\"}; os=${os%\"}; os=${os#\'}; os=${os%\'}
    printf 'OS: %s\n' "${os:-unavailable}"
else
    printf 'OS (macOS): %s\n' "$(capture sw_vers -productVersion || printf unavailable)"
fi
printf 'Kernel: %s\n' "$(capture uname -sr || printf unavailable)"
root=$(capture git_read rev-parse --show-toplevel)
if (($? != 0)); then
    [[ $root == 'fatal: not a git repository (or any'* ]] && printf 'Git repository: no\n' || printf 'Git repository: unavailable\n'
    exit 0
fi
printf 'Git repository: yes\n'
[[ $root == "$PWD" ]] || printf 'Git worktree: %s\n' "$root"
if dir=$(capture git_read rev-parse --absolute-git-dir) && common=$(capture git_read rev-parse --path-format=absolute --git-common-dir); then
    [[ $dir == "$common" ]] || printf 'Linked worktree: true\n'
else
    printf 'Linked worktree: unavailable\n'
fi
branch=$(capture git_read symbolic-ref --quiet --short HEAD); branch_ok=$?
head=$(capture git_read rev-parse --verify --short=9 HEAD); head_ok=$?
if ((branch_ok != 0)); then
    [[ $head_ok == 0 ]] && branch=detached || branch=unavailable
fi
if ((head_ok != 0)); then
    [[ $branch_ok == 0 ]] && head=unborn || head=unavailable
fi
printf 'Branch: %s\nHEAD: %s\n' "$branch" "$head"
refs=$(capture git_read for-each-ref '--format=%(refname) %(symref)' refs/remotes) || refs=unavailable
printf 'Default branch: %s\n' "$(printf '%s\n' "$refs" | awk '
    $1~/\/HEAD$/ && $2~/^refs\/remotes\// {
        p=$1; sub(/HEAD$/,"",p)
        if(index($2,p)==1) names[substr($2,length(p)+1)]=1
    }
    END {for(b in names){n++;value=b};print $0=="[truncated]"?"unavailable (truncated)":n==1?value:"unavailable"}')"
hooks=$(capture git --no-lazy-fetch --no-optional-locks config --get core.hooksPath); code=$?
if ((code != 0)); then [[ $code == 1 && -z $hooks ]] && hooks='unset (Git default)' || hooks=unavailable; fi
printf 'Git hooks path: %s\n' "$hooks"
excludes=$(capture git --no-lazy-fetch --no-optional-locks config --path --get core.excludesFile) || excludes=
status=$(capture git_read -c "core.excludesFile=$excludes" status --porcelain=v1 --untracked-files=normal --ignore-submodules=none) || status=unavailable
printf 'Status:\n%s\n' "${status:-clean}" | awk 'NR<=33{print} NR==34{print "[status truncated]"}'
if ((head_ok == 0)); then
    remotes=$(capture git_read for-each-ref --contains=HEAD '--format=%(refname:strip=2) %(symref)' refs/remotes) || remotes=unavailable
    printf 'HEAD in local remote-tracking branches:\n'
    printf '%s\n' "${remotes:-none}" | awk 'NF==1{n++;if(n<=16)print;else if(n==17)print "[truncated]"}'
    recent=$(capture git_read log -5 --abbrev=9 '--format=%h %s' --no-decorate) || recent=unavailable
    printf 'Recent commits:\n%s\n' "$recent"
    if [[ -n $refs && $refs != unavailable ]]; then
        count=$(capture git_read rev-list --count HEAD --not --remotes) && newest=$(capture git_read rev-list --max-count=5 HEAD --not --remotes) && [[ $recent != unavailable && $recent != *'[truncated]'* ]] || count=unavailable
        listed=0
        while IFS= read -r sha; do
            [[ -n $sha && $'\n'$recent == *$'\n'"${sha:0:9} "* ]] && ((listed+=1))
        done <<< "$newest"
        if [[ $count =~ ^[0-9]+$ ]]; then
            ((count>listed)) && printf '(... %s more unpushed commits)\n' "$((count-listed))"
        else
            printf 'Further unpushed commits: unavailable\n'
        fi
    fi
fi
exit 0
