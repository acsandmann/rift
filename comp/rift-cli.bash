_rift-cli() {
    local i cur prev opts cmd
    COMPREPLY=()
    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
        cur="$2"
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
    fi
    prev="$3"
    cmd=""
    opts=""

    for i in "${COMP_WORDS[@]:0:COMP_CWORD}"
    do
        case "${cmd},${i}" in
            ",$1")
                cmd="rift__cli"
                ;;
            rift__cli,execute)
                cmd="rift__cli__subcmd__execute"
                ;;
            rift__cli,help)
                cmd="rift__cli__subcmd__help"
                ;;
            rift__cli,query)
                cmd="rift__cli__subcmd__query"
                ;;
            rift__cli,service)
                cmd="rift__cli__subcmd__service"
                ;;
            rift__cli,subscribe)
                cmd="rift__cli__subcmd__subscribe"
                ;;
            rift__cli,verify)
                cmd="rift__cli__subcmd__verify"
                ;;
            rift__cli__subcmd__execute,config)
                cmd="rift__cli__subcmd__execute__subcmd__config"
                ;;
            rift__cli__subcmd__execute,debug)
                cmd="rift__cli__subcmd__execute__subcmd__debug"
                ;;
            rift__cli__subcmd__execute,display)
                cmd="rift__cli__subcmd__execute__subcmd__display"
                ;;
            rift__cli__subcmd__execute,help)
                cmd="rift__cli__subcmd__execute__subcmd__help"
                ;;
            rift__cli__subcmd__execute,layout)
                cmd="rift__cli__subcmd__execute__subcmd__layout"
                ;;
            rift__cli__subcmd__execute,mission-control)
                cmd="rift__cli__subcmd__execute__subcmd__mission__subcmd__control"
                ;;
            rift__cli__subcmd__execute,save-and-exit)
                cmd="rift__cli__subcmd__execute__subcmd__save__subcmd__and__subcmd__exit"
                ;;
            rift__cli__subcmd__execute,serialize)
                cmd="rift__cli__subcmd__execute__subcmd__serialize"
                ;;
            rift__cli__subcmd__execute,show-timing)
                cmd="rift__cli__subcmd__execute__subcmd__show__subcmd__timing"
                ;;
            rift__cli__subcmd__execute,toggle-space-activated)
                cmd="rift__cli__subcmd__execute__subcmd__toggle__subcmd__space__subcmd__activated"
                ;;
            rift__cli__subcmd__execute,window)
                cmd="rift__cli__subcmd__execute__subcmd__window"
                ;;
            rift__cli__subcmd__execute,workspace)
                cmd="rift__cli__subcmd__execute__subcmd__workspace"
                ;;
            rift__cli__subcmd__execute__subcmd__config,get)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__get"
                ;;
            rift__cli__subcmd__execute__subcmd__config,help)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__help"
                ;;
            rift__cli__subcmd__execute__subcmd__config,reload)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__reload"
                ;;
            rift__cli__subcmd__execute__subcmd__config,save)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__save"
                ;;
            rift__cli__subcmd__execute__subcmd__config,set)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__set"
                ;;
            rift__cli__subcmd__execute__subcmd__config,set-animate)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__animate"
                ;;
            rift__cli__subcmd__execute__subcmd__config,set-animation-duration)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__animation__subcmd__duration"
                ;;
            rift__cli__subcmd__execute__subcmd__config,set-animation-easing)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__animation__subcmd__easing"
                ;;
            rift__cli__subcmd__execute__subcmd__config,set-animation-fps)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__animation__subcmd__fps"
                ;;
            rift__cli__subcmd__execute__subcmd__config,set-focus-follows-mouse)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__focus__subcmd__follows__subcmd__mouse"
                ;;
            rift__cli__subcmd__execute__subcmd__config,set-inner-gaps)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__inner__subcmd__gaps"
                ;;
            rift__cli__subcmd__execute__subcmd__config,set-mouse-follows-focus)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__mouse__subcmd__follows__subcmd__focus"
                ;;
            rift__cli__subcmd__execute__subcmd__config,set-mouse-hides-on-focus)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__mouse__subcmd__hides__subcmd__on__subcmd__focus"
                ;;
            rift__cli__subcmd__execute__subcmd__config,set-outer-gaps)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__outer__subcmd__gaps"
                ;;
            rift__cli__subcmd__execute__subcmd__config,set-stack-default-orientation)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__stack__subcmd__default__subcmd__orientation"
                ;;
            rift__cli__subcmd__execute__subcmd__config,set-stack-offset)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__stack__subcmd__offset"
                ;;
            rift__cli__subcmd__execute__subcmd__config,set-workspace-names)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__workspace__subcmd__names"
                ;;
            rift__cli__subcmd__execute__subcmd__config__subcmd__help,get)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__get"
                ;;
            rift__cli__subcmd__execute__subcmd__config__subcmd__help,help)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__help"
                ;;
            rift__cli__subcmd__execute__subcmd__config__subcmd__help,reload)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__reload"
                ;;
            rift__cli__subcmd__execute__subcmd__config__subcmd__help,save)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__save"
                ;;
            rift__cli__subcmd__execute__subcmd__config__subcmd__help,set)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set"
                ;;
            rift__cli__subcmd__execute__subcmd__config__subcmd__help,set-animate)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__animate"
                ;;
            rift__cli__subcmd__execute__subcmd__config__subcmd__help,set-animation-duration)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__animation__subcmd__duration"
                ;;
            rift__cli__subcmd__execute__subcmd__config__subcmd__help,set-animation-easing)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__animation__subcmd__easing"
                ;;
            rift__cli__subcmd__execute__subcmd__config__subcmd__help,set-animation-fps)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__animation__subcmd__fps"
                ;;
            rift__cli__subcmd__execute__subcmd__config__subcmd__help,set-focus-follows-mouse)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__focus__subcmd__follows__subcmd__mouse"
                ;;
            rift__cli__subcmd__execute__subcmd__config__subcmd__help,set-inner-gaps)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__inner__subcmd__gaps"
                ;;
            rift__cli__subcmd__execute__subcmd__config__subcmd__help,set-mouse-follows-focus)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__mouse__subcmd__follows__subcmd__focus"
                ;;
            rift__cli__subcmd__execute__subcmd__config__subcmd__help,set-mouse-hides-on-focus)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__mouse__subcmd__hides__subcmd__on__subcmd__focus"
                ;;
            rift__cli__subcmd__execute__subcmd__config__subcmd__help,set-outer-gaps)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__outer__subcmd__gaps"
                ;;
            rift__cli__subcmd__execute__subcmd__config__subcmd__help,set-stack-default-orientation)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__stack__subcmd__default__subcmd__orientation"
                ;;
            rift__cli__subcmd__execute__subcmd__config__subcmd__help,set-stack-offset)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__stack__subcmd__offset"
                ;;
            rift__cli__subcmd__execute__subcmd__config__subcmd__help,set-workspace-names)
                cmd="rift__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__workspace__subcmd__names"
                ;;
            rift__cli__subcmd__execute__subcmd__display,focus)
                cmd="rift__cli__subcmd__execute__subcmd__display__subcmd__focus"
                ;;
            rift__cli__subcmd__execute__subcmd__display,help)
                cmd="rift__cli__subcmd__execute__subcmd__display__subcmd__help"
                ;;
            rift__cli__subcmd__execute__subcmd__display,move-mouse-to-index)
                cmd="rift__cli__subcmd__execute__subcmd__display__subcmd__move__subcmd__mouse__subcmd__to__subcmd__index"
                ;;
            rift__cli__subcmd__execute__subcmd__display,move-mouse-to-uuid)
                cmd="rift__cli__subcmd__execute__subcmd__display__subcmd__move__subcmd__mouse__subcmd__to__subcmd__uuid"
                ;;
            rift__cli__subcmd__execute__subcmd__display,move-window)
                cmd="rift__cli__subcmd__execute__subcmd__display__subcmd__move__subcmd__window"
                ;;
            rift__cli__subcmd__execute__subcmd__display__subcmd__help,focus)
                cmd="rift__cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__focus"
                ;;
            rift__cli__subcmd__execute__subcmd__display__subcmd__help,help)
                cmd="rift__cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__help"
                ;;
            rift__cli__subcmd__execute__subcmd__display__subcmd__help,move-mouse-to-index)
                cmd="rift__cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__move__subcmd__mouse__subcmd__to__subcmd__index"
                ;;
            rift__cli__subcmd__execute__subcmd__display__subcmd__help,move-mouse-to-uuid)
                cmd="rift__cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__move__subcmd__mouse__subcmd__to__subcmd__uuid"
                ;;
            rift__cli__subcmd__execute__subcmd__display__subcmd__help,move-window)
                cmd="rift__cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__move__subcmd__window"
                ;;
            rift__cli__subcmd__execute__subcmd__help,config)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__config"
                ;;
            rift__cli__subcmd__execute__subcmd__help,debug)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__debug"
                ;;
            rift__cli__subcmd__execute__subcmd__help,display)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__display"
                ;;
            rift__cli__subcmd__execute__subcmd__help,help)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__help"
                ;;
            rift__cli__subcmd__execute__subcmd__help,layout)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__layout"
                ;;
            rift__cli__subcmd__execute__subcmd__help,mission-control)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__mission__subcmd__control"
                ;;
            rift__cli__subcmd__execute__subcmd__help,save-and-exit)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__save__subcmd__and__subcmd__exit"
                ;;
            rift__cli__subcmd__execute__subcmd__help,serialize)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__serialize"
                ;;
            rift__cli__subcmd__execute__subcmd__help,show-timing)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__show__subcmd__timing"
                ;;
            rift__cli__subcmd__execute__subcmd__help,toggle-space-activated)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__toggle__subcmd__space__subcmd__activated"
                ;;
            rift__cli__subcmd__execute__subcmd__help,window)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__window"
                ;;
            rift__cli__subcmd__execute__subcmd__help,workspace)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__workspace"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__config,get)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__get"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__config,reload)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__reload"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__config,save)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__save"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__config,set)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__config,set-animate)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__animate"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__config,set-animation-duration)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__animation__subcmd__duration"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__config,set-animation-easing)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__animation__subcmd__easing"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__config,set-animation-fps)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__animation__subcmd__fps"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__config,set-focus-follows-mouse)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__focus__subcmd__follows__subcmd__mouse"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__config,set-inner-gaps)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__inner__subcmd__gaps"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__config,set-mouse-follows-focus)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__mouse__subcmd__follows__subcmd__focus"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__config,set-mouse-hides-on-focus)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__mouse__subcmd__hides__subcmd__on__subcmd__focus"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__config,set-outer-gaps)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__outer__subcmd__gaps"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__config,set-stack-default-orientation)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__stack__subcmd__default__subcmd__orientation"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__config,set-stack-offset)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__stack__subcmd__offset"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__config,set-workspace-names)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__workspace__subcmd__names"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__display,focus)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__display__subcmd__focus"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__display,move-mouse-to-index)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__display__subcmd__move__subcmd__mouse__subcmd__to__subcmd__index"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__display,move-mouse-to-uuid)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__display__subcmd__move__subcmd__mouse__subcmd__to__subcmd__uuid"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__display,move-window)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__display__subcmd__move__subcmd__window"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__layout,adjust-master-count)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__adjust__subcmd__master__subcmd__count"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__layout,adjust-master-ratio)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__adjust__subcmd__master__subcmd__ratio"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__layout,ascend)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__ascend"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__layout,center-selection)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__center__subcmd__selection"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__layout,descend)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__descend"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__layout,join-window)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__join__subcmd__window"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__layout,move-node)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__move__subcmd__node"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__layout,promote-to-master)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__promote__subcmd__to__subcmd__master"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__layout,scroll-strip)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__scroll__subcmd__strip"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__layout,snap-strip)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__snap__subcmd__strip"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__layout,swap-master-stack)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__swap__subcmd__master__subcmd__stack"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__layout,swap-windows)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__swap__subcmd__windows"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__layout,toggle-focus-float)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__toggle__subcmd__focus__subcmd__float"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__layout,toggle-orientation)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__toggle__subcmd__orientation"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__layout,toggle-stack)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__toggle__subcmd__stack"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__layout,unjoin)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__unjoin"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__mission__subcmd__control,dismiss)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__mission__subcmd__control__subcmd__dismiss"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__mission__subcmd__control,show-all)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__mission__subcmd__control__subcmd__show__subcmd__all"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__mission__subcmd__control,show-current)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__mission__subcmd__control__subcmd__show__subcmd__current"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__window,add-scratchpad)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__add__subcmd__scratchpad"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__window,close)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__close"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__window,focus)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__focus"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__window,next)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__next"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__window,prev)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__prev"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__window,resize-by)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__resize__subcmd__by"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__window,resize-grow)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__resize__subcmd__grow"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__window,resize-shrink)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__resize__subcmd__shrink"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__window,toggle-float)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__toggle__subcmd__float"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__window,toggle-fullscreen)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__toggle__subcmd__fullscreen"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__window,toggle-fullscreen-within-gaps)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__toggle__subcmd__fullscreen__subcmd__within__subcmd__gaps"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__window,toggle-scratchpad)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__toggle__subcmd__scratchpad"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__workspace,create)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__create"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__workspace,last)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__last"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__workspace,move-window)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__move__subcmd__window"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__workspace,next)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__next"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__workspace,prev)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__prev"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__workspace,set-layout)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__set__subcmd__layout"
                ;;
            rift__cli__subcmd__execute__subcmd__help__subcmd__workspace,switch)
                cmd="rift__cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__switch"
                ;;
            rift__cli__subcmd__execute__subcmd__layout,adjust-master-count)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__adjust__subcmd__master__subcmd__count"
                ;;
            rift__cli__subcmd__execute__subcmd__layout,adjust-master-ratio)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__adjust__subcmd__master__subcmd__ratio"
                ;;
            rift__cli__subcmd__execute__subcmd__layout,ascend)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__ascend"
                ;;
            rift__cli__subcmd__execute__subcmd__layout,center-selection)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__center__subcmd__selection"
                ;;
            rift__cli__subcmd__execute__subcmd__layout,descend)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__descend"
                ;;
            rift__cli__subcmd__execute__subcmd__layout,help)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__help"
                ;;
            rift__cli__subcmd__execute__subcmd__layout,join-window)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__join__subcmd__window"
                ;;
            rift__cli__subcmd__execute__subcmd__layout,move-node)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__move__subcmd__node"
                ;;
            rift__cli__subcmd__execute__subcmd__layout,promote-to-master)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__promote__subcmd__to__subcmd__master"
                ;;
            rift__cli__subcmd__execute__subcmd__layout,scroll-strip)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__scroll__subcmd__strip"
                ;;
            rift__cli__subcmd__execute__subcmd__layout,snap-strip)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__snap__subcmd__strip"
                ;;
            rift__cli__subcmd__execute__subcmd__layout,swap-master-stack)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__swap__subcmd__master__subcmd__stack"
                ;;
            rift__cli__subcmd__execute__subcmd__layout,swap-windows)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__swap__subcmd__windows"
                ;;
            rift__cli__subcmd__execute__subcmd__layout,toggle-focus-float)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__toggle__subcmd__focus__subcmd__float"
                ;;
            rift__cli__subcmd__execute__subcmd__layout,toggle-orientation)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__toggle__subcmd__orientation"
                ;;
            rift__cli__subcmd__execute__subcmd__layout,toggle-stack)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__toggle__subcmd__stack"
                ;;
            rift__cli__subcmd__execute__subcmd__layout,unjoin)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__unjoin"
                ;;
            rift__cli__subcmd__execute__subcmd__layout__subcmd__help,adjust-master-count)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__adjust__subcmd__master__subcmd__count"
                ;;
            rift__cli__subcmd__execute__subcmd__layout__subcmd__help,adjust-master-ratio)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__adjust__subcmd__master__subcmd__ratio"
                ;;
            rift__cli__subcmd__execute__subcmd__layout__subcmd__help,ascend)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__ascend"
                ;;
            rift__cli__subcmd__execute__subcmd__layout__subcmd__help,center-selection)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__center__subcmd__selection"
                ;;
            rift__cli__subcmd__execute__subcmd__layout__subcmd__help,descend)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__descend"
                ;;
            rift__cli__subcmd__execute__subcmd__layout__subcmd__help,help)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__help"
                ;;
            rift__cli__subcmd__execute__subcmd__layout__subcmd__help,join-window)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__join__subcmd__window"
                ;;
            rift__cli__subcmd__execute__subcmd__layout__subcmd__help,move-node)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__move__subcmd__node"
                ;;
            rift__cli__subcmd__execute__subcmd__layout__subcmd__help,promote-to-master)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__promote__subcmd__to__subcmd__master"
                ;;
            rift__cli__subcmd__execute__subcmd__layout__subcmd__help,scroll-strip)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__scroll__subcmd__strip"
                ;;
            rift__cli__subcmd__execute__subcmd__layout__subcmd__help,snap-strip)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__snap__subcmd__strip"
                ;;
            rift__cli__subcmd__execute__subcmd__layout__subcmd__help,swap-master-stack)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__swap__subcmd__master__subcmd__stack"
                ;;
            rift__cli__subcmd__execute__subcmd__layout__subcmd__help,swap-windows)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__swap__subcmd__windows"
                ;;
            rift__cli__subcmd__execute__subcmd__layout__subcmd__help,toggle-focus-float)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__toggle__subcmd__focus__subcmd__float"
                ;;
            rift__cli__subcmd__execute__subcmd__layout__subcmd__help,toggle-orientation)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__toggle__subcmd__orientation"
                ;;
            rift__cli__subcmd__execute__subcmd__layout__subcmd__help,toggle-stack)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__toggle__subcmd__stack"
                ;;
            rift__cli__subcmd__execute__subcmd__layout__subcmd__help,unjoin)
                cmd="rift__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__unjoin"
                ;;
            rift__cli__subcmd__execute__subcmd__mission__subcmd__control,dismiss)
                cmd="rift__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__dismiss"
                ;;
            rift__cli__subcmd__execute__subcmd__mission__subcmd__control,help)
                cmd="rift__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__help"
                ;;
            rift__cli__subcmd__execute__subcmd__mission__subcmd__control,show-all)
                cmd="rift__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__show__subcmd__all"
                ;;
            rift__cli__subcmd__execute__subcmd__mission__subcmd__control,show-current)
                cmd="rift__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__show__subcmd__current"
                ;;
            rift__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__help,dismiss)
                cmd="rift__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__help__subcmd__dismiss"
                ;;
            rift__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__help,help)
                cmd="rift__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__help__subcmd__help"
                ;;
            rift__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__help,show-all)
                cmd="rift__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__help__subcmd__show__subcmd__all"
                ;;
            rift__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__help,show-current)
                cmd="rift__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__help__subcmd__show__subcmd__current"
                ;;
            rift__cli__subcmd__execute__subcmd__window,add-scratchpad)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__add__subcmd__scratchpad"
                ;;
            rift__cli__subcmd__execute__subcmd__window,close)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__close"
                ;;
            rift__cli__subcmd__execute__subcmd__window,focus)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__focus"
                ;;
            rift__cli__subcmd__execute__subcmd__window,help)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__help"
                ;;
            rift__cli__subcmd__execute__subcmd__window,next)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__next"
                ;;
            rift__cli__subcmd__execute__subcmd__window,prev)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__prev"
                ;;
            rift__cli__subcmd__execute__subcmd__window,resize-by)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__resize__subcmd__by"
                ;;
            rift__cli__subcmd__execute__subcmd__window,resize-grow)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__resize__subcmd__grow"
                ;;
            rift__cli__subcmd__execute__subcmd__window,resize-shrink)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__resize__subcmd__shrink"
                ;;
            rift__cli__subcmd__execute__subcmd__window,toggle-float)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__toggle__subcmd__float"
                ;;
            rift__cli__subcmd__execute__subcmd__window,toggle-fullscreen)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__toggle__subcmd__fullscreen"
                ;;
            rift__cli__subcmd__execute__subcmd__window,toggle-fullscreen-within-gaps)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__toggle__subcmd__fullscreen__subcmd__within__subcmd__gaps"
                ;;
            rift__cli__subcmd__execute__subcmd__window,toggle-scratchpad)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__toggle__subcmd__scratchpad"
                ;;
            rift__cli__subcmd__execute__subcmd__window__subcmd__help,add-scratchpad)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__add__subcmd__scratchpad"
                ;;
            rift__cli__subcmd__execute__subcmd__window__subcmd__help,close)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__close"
                ;;
            rift__cli__subcmd__execute__subcmd__window__subcmd__help,focus)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__focus"
                ;;
            rift__cli__subcmd__execute__subcmd__window__subcmd__help,help)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__help"
                ;;
            rift__cli__subcmd__execute__subcmd__window__subcmd__help,next)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__next"
                ;;
            rift__cli__subcmd__execute__subcmd__window__subcmd__help,prev)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__prev"
                ;;
            rift__cli__subcmd__execute__subcmd__window__subcmd__help,resize-by)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__resize__subcmd__by"
                ;;
            rift__cli__subcmd__execute__subcmd__window__subcmd__help,resize-grow)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__resize__subcmd__grow"
                ;;
            rift__cli__subcmd__execute__subcmd__window__subcmd__help,resize-shrink)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__resize__subcmd__shrink"
                ;;
            rift__cli__subcmd__execute__subcmd__window__subcmd__help,toggle-float)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__toggle__subcmd__float"
                ;;
            rift__cli__subcmd__execute__subcmd__window__subcmd__help,toggle-fullscreen)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__toggle__subcmd__fullscreen"
                ;;
            rift__cli__subcmd__execute__subcmd__window__subcmd__help,toggle-fullscreen-within-gaps)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__toggle__subcmd__fullscreen__subcmd__within__subcmd__gaps"
                ;;
            rift__cli__subcmd__execute__subcmd__window__subcmd__help,toggle-scratchpad)
                cmd="rift__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__toggle__subcmd__scratchpad"
                ;;
            rift__cli__subcmd__execute__subcmd__workspace,create)
                cmd="rift__cli__subcmd__execute__subcmd__workspace__subcmd__create"
                ;;
            rift__cli__subcmd__execute__subcmd__workspace,help)
                cmd="rift__cli__subcmd__execute__subcmd__workspace__subcmd__help"
                ;;
            rift__cli__subcmd__execute__subcmd__workspace,last)
                cmd="rift__cli__subcmd__execute__subcmd__workspace__subcmd__last"
                ;;
            rift__cli__subcmd__execute__subcmd__workspace,move-window)
                cmd="rift__cli__subcmd__execute__subcmd__workspace__subcmd__move__subcmd__window"
                ;;
            rift__cli__subcmd__execute__subcmd__workspace,next)
                cmd="rift__cli__subcmd__execute__subcmd__workspace__subcmd__next"
                ;;
            rift__cli__subcmd__execute__subcmd__workspace,prev)
                cmd="rift__cli__subcmd__execute__subcmd__workspace__subcmd__prev"
                ;;
            rift__cli__subcmd__execute__subcmd__workspace,set-layout)
                cmd="rift__cli__subcmd__execute__subcmd__workspace__subcmd__set__subcmd__layout"
                ;;
            rift__cli__subcmd__execute__subcmd__workspace,switch)
                cmd="rift__cli__subcmd__execute__subcmd__workspace__subcmd__switch"
                ;;
            rift__cli__subcmd__execute__subcmd__workspace__subcmd__help,create)
                cmd="rift__cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__create"
                ;;
            rift__cli__subcmd__execute__subcmd__workspace__subcmd__help,help)
                cmd="rift__cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__help"
                ;;
            rift__cli__subcmd__execute__subcmd__workspace__subcmd__help,last)
                cmd="rift__cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__last"
                ;;
            rift__cli__subcmd__execute__subcmd__workspace__subcmd__help,move-window)
                cmd="rift__cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__move__subcmd__window"
                ;;
            rift__cli__subcmd__execute__subcmd__workspace__subcmd__help,next)
                cmd="rift__cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__next"
                ;;
            rift__cli__subcmd__execute__subcmd__workspace__subcmd__help,prev)
                cmd="rift__cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__prev"
                ;;
            rift__cli__subcmd__execute__subcmd__workspace__subcmd__help,set-layout)
                cmd="rift__cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__set__subcmd__layout"
                ;;
            rift__cli__subcmd__execute__subcmd__workspace__subcmd__help,switch)
                cmd="rift__cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__switch"
                ;;
            rift__cli__subcmd__help,execute)
                cmd="rift__cli__subcmd__help__subcmd__execute"
                ;;
            rift__cli__subcmd__help,help)
                cmd="rift__cli__subcmd__help__subcmd__help"
                ;;
            rift__cli__subcmd__help,query)
                cmd="rift__cli__subcmd__help__subcmd__query"
                ;;
            rift__cli__subcmd__help,service)
                cmd="rift__cli__subcmd__help__subcmd__service"
                ;;
            rift__cli__subcmd__help,subscribe)
                cmd="rift__cli__subcmd__help__subcmd__subscribe"
                ;;
            rift__cli__subcmd__help,verify)
                cmd="rift__cli__subcmd__help__subcmd__verify"
                ;;
            rift__cli__subcmd__help__subcmd__execute,config)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__config"
                ;;
            rift__cli__subcmd__help__subcmd__execute,debug)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__debug"
                ;;
            rift__cli__subcmd__help__subcmd__execute,display)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__display"
                ;;
            rift__cli__subcmd__help__subcmd__execute,layout)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__layout"
                ;;
            rift__cli__subcmd__help__subcmd__execute,mission-control)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__mission__subcmd__control"
                ;;
            rift__cli__subcmd__help__subcmd__execute,save-and-exit)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__save__subcmd__and__subcmd__exit"
                ;;
            rift__cli__subcmd__help__subcmd__execute,serialize)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__serialize"
                ;;
            rift__cli__subcmd__help__subcmd__execute,show-timing)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__show__subcmd__timing"
                ;;
            rift__cli__subcmd__help__subcmd__execute,toggle-space-activated)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__toggle__subcmd__space__subcmd__activated"
                ;;
            rift__cli__subcmd__help__subcmd__execute,window)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__window"
                ;;
            rift__cli__subcmd__help__subcmd__execute,workspace)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__workspace"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__config,get)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__get"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__config,reload)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__reload"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__config,save)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__save"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__config,set)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__config,set-animate)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__animate"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__config,set-animation-duration)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__animation__subcmd__duration"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__config,set-animation-easing)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__animation__subcmd__easing"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__config,set-animation-fps)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__animation__subcmd__fps"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__config,set-focus-follows-mouse)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__focus__subcmd__follows__subcmd__mouse"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__config,set-inner-gaps)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__inner__subcmd__gaps"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__config,set-mouse-follows-focus)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__mouse__subcmd__follows__subcmd__focus"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__config,set-mouse-hides-on-focus)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__mouse__subcmd__hides__subcmd__on__subcmd__focus"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__config,set-outer-gaps)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__outer__subcmd__gaps"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__config,set-stack-default-orientation)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__stack__subcmd__default__subcmd__orientation"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__config,set-stack-offset)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__stack__subcmd__offset"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__config,set-workspace-names)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__workspace__subcmd__names"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__display,focus)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__display__subcmd__focus"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__display,move-mouse-to-index)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__display__subcmd__move__subcmd__mouse__subcmd__to__subcmd__index"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__display,move-mouse-to-uuid)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__display__subcmd__move__subcmd__mouse__subcmd__to__subcmd__uuid"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__display,move-window)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__display__subcmd__move__subcmd__window"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__layout,adjust-master-count)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__adjust__subcmd__master__subcmd__count"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__layout,adjust-master-ratio)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__adjust__subcmd__master__subcmd__ratio"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__layout,ascend)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__ascend"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__layout,center-selection)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__center__subcmd__selection"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__layout,descend)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__descend"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__layout,join-window)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__join__subcmd__window"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__layout,move-node)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__move__subcmd__node"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__layout,promote-to-master)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__promote__subcmd__to__subcmd__master"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__layout,scroll-strip)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__scroll__subcmd__strip"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__layout,snap-strip)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__snap__subcmd__strip"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__layout,swap-master-stack)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__swap__subcmd__master__subcmd__stack"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__layout,swap-windows)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__swap__subcmd__windows"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__layout,toggle-focus-float)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__toggle__subcmd__focus__subcmd__float"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__layout,toggle-orientation)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__toggle__subcmd__orientation"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__layout,toggle-stack)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__toggle__subcmd__stack"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__layout,unjoin)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__unjoin"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__mission__subcmd__control,dismiss)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__mission__subcmd__control__subcmd__dismiss"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__mission__subcmd__control,show-all)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__mission__subcmd__control__subcmd__show__subcmd__all"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__mission__subcmd__control,show-current)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__mission__subcmd__control__subcmd__show__subcmd__current"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__window,add-scratchpad)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__add__subcmd__scratchpad"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__window,close)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__close"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__window,focus)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__focus"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__window,next)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__next"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__window,prev)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__prev"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__window,resize-by)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__resize__subcmd__by"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__window,resize-grow)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__resize__subcmd__grow"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__window,resize-shrink)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__resize__subcmd__shrink"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__window,toggle-float)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__toggle__subcmd__float"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__window,toggle-fullscreen)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__toggle__subcmd__fullscreen"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__window,toggle-fullscreen-within-gaps)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__toggle__subcmd__fullscreen__subcmd__within__subcmd__gaps"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__window,toggle-scratchpad)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__toggle__subcmd__scratchpad"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__workspace,create)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__create"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__workspace,last)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__last"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__workspace,move-window)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__move__subcmd__window"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__workspace,next)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__next"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__workspace,prev)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__prev"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__workspace,set-layout)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__set__subcmd__layout"
                ;;
            rift__cli__subcmd__help__subcmd__execute__subcmd__workspace,switch)
                cmd="rift__cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__switch"
                ;;
            rift__cli__subcmd__help__subcmd__query,applications)
                cmd="rift__cli__subcmd__help__subcmd__query__subcmd__applications"
                ;;
            rift__cli__subcmd__help__subcmd__query,displays)
                cmd="rift__cli__subcmd__help__subcmd__query__subcmd__displays"
                ;;
            rift__cli__subcmd__help__subcmd__query,layout)
                cmd="rift__cli__subcmd__help__subcmd__query__subcmd__layout"
                ;;
            rift__cli__subcmd__help__subcmd__query,metrics)
                cmd="rift__cli__subcmd__help__subcmd__query__subcmd__metrics"
                ;;
            rift__cli__subcmd__help__subcmd__query,window)
                cmd="rift__cli__subcmd__help__subcmd__query__subcmd__window"
                ;;
            rift__cli__subcmd__help__subcmd__query,windows)
                cmd="rift__cli__subcmd__help__subcmd__query__subcmd__windows"
                ;;
            rift__cli__subcmd__help__subcmd__query,workspace-layout)
                cmd="rift__cli__subcmd__help__subcmd__query__subcmd__workspace__subcmd__layout"
                ;;
            rift__cli__subcmd__help__subcmd__query,workspaces)
                cmd="rift__cli__subcmd__help__subcmd__query__subcmd__workspaces"
                ;;
            rift__cli__subcmd__help__subcmd__service,install)
                cmd="rift__cli__subcmd__help__subcmd__service__subcmd__install"
                ;;
            rift__cli__subcmd__help__subcmd__service,restart)
                cmd="rift__cli__subcmd__help__subcmd__service__subcmd__restart"
                ;;
            rift__cli__subcmd__help__subcmd__service,start)
                cmd="rift__cli__subcmd__help__subcmd__service__subcmd__start"
                ;;
            rift__cli__subcmd__help__subcmd__service,stop)
                cmd="rift__cli__subcmd__help__subcmd__service__subcmd__stop"
                ;;
            rift__cli__subcmd__help__subcmd__service,uninstall)
                cmd="rift__cli__subcmd__help__subcmd__service__subcmd__uninstall"
                ;;
            rift__cli__subcmd__help__subcmd__subscribe,cli)
                cmd="rift__cli__subcmd__help__subcmd__subscribe__subcmd__cli"
                ;;
            rift__cli__subcmd__help__subcmd__subscribe,list-cli)
                cmd="rift__cli__subcmd__help__subcmd__subscribe__subcmd__list__subcmd__cli"
                ;;
            rift__cli__subcmd__help__subcmd__subscribe,mach)
                cmd="rift__cli__subcmd__help__subcmd__subscribe__subcmd__mach"
                ;;
            rift__cli__subcmd__help__subcmd__subscribe,unsub-cli)
                cmd="rift__cli__subcmd__help__subcmd__subscribe__subcmd__unsub__subcmd__cli"
                ;;
            rift__cli__subcmd__help__subcmd__subscribe,unsub-mach)
                cmd="rift__cli__subcmd__help__subcmd__subscribe__subcmd__unsub__subcmd__mach"
                ;;
            rift__cli__subcmd__query,applications)
                cmd="rift__cli__subcmd__query__subcmd__applications"
                ;;
            rift__cli__subcmd__query,displays)
                cmd="rift__cli__subcmd__query__subcmd__displays"
                ;;
            rift__cli__subcmd__query,help)
                cmd="rift__cli__subcmd__query__subcmd__help"
                ;;
            rift__cli__subcmd__query,layout)
                cmd="rift__cli__subcmd__query__subcmd__layout"
                ;;
            rift__cli__subcmd__query,metrics)
                cmd="rift__cli__subcmd__query__subcmd__metrics"
                ;;
            rift__cli__subcmd__query,window)
                cmd="rift__cli__subcmd__query__subcmd__window"
                ;;
            rift__cli__subcmd__query,windows)
                cmd="rift__cli__subcmd__query__subcmd__windows"
                ;;
            rift__cli__subcmd__query,workspace-layout)
                cmd="rift__cli__subcmd__query__subcmd__workspace__subcmd__layout"
                ;;
            rift__cli__subcmd__query,workspaces)
                cmd="rift__cli__subcmd__query__subcmd__workspaces"
                ;;
            rift__cli__subcmd__query__subcmd__help,applications)
                cmd="rift__cli__subcmd__query__subcmd__help__subcmd__applications"
                ;;
            rift__cli__subcmd__query__subcmd__help,displays)
                cmd="rift__cli__subcmd__query__subcmd__help__subcmd__displays"
                ;;
            rift__cli__subcmd__query__subcmd__help,help)
                cmd="rift__cli__subcmd__query__subcmd__help__subcmd__help"
                ;;
            rift__cli__subcmd__query__subcmd__help,layout)
                cmd="rift__cli__subcmd__query__subcmd__help__subcmd__layout"
                ;;
            rift__cli__subcmd__query__subcmd__help,metrics)
                cmd="rift__cli__subcmd__query__subcmd__help__subcmd__metrics"
                ;;
            rift__cli__subcmd__query__subcmd__help,window)
                cmd="rift__cli__subcmd__query__subcmd__help__subcmd__window"
                ;;
            rift__cli__subcmd__query__subcmd__help,windows)
                cmd="rift__cli__subcmd__query__subcmd__help__subcmd__windows"
                ;;
            rift__cli__subcmd__query__subcmd__help,workspace-layout)
                cmd="rift__cli__subcmd__query__subcmd__help__subcmd__workspace__subcmd__layout"
                ;;
            rift__cli__subcmd__query__subcmd__help,workspaces)
                cmd="rift__cli__subcmd__query__subcmd__help__subcmd__workspaces"
                ;;
            rift__cli__subcmd__service,help)
                cmd="rift__cli__subcmd__service__subcmd__help"
                ;;
            rift__cli__subcmd__service,install)
                cmd="rift__cli__subcmd__service__subcmd__install"
                ;;
            rift__cli__subcmd__service,restart)
                cmd="rift__cli__subcmd__service__subcmd__restart"
                ;;
            rift__cli__subcmd__service,start)
                cmd="rift__cli__subcmd__service__subcmd__start"
                ;;
            rift__cli__subcmd__service,stop)
                cmd="rift__cli__subcmd__service__subcmd__stop"
                ;;
            rift__cli__subcmd__service,uninstall)
                cmd="rift__cli__subcmd__service__subcmd__uninstall"
                ;;
            rift__cli__subcmd__service__subcmd__help,help)
                cmd="rift__cli__subcmd__service__subcmd__help__subcmd__help"
                ;;
            rift__cli__subcmd__service__subcmd__help,install)
                cmd="rift__cli__subcmd__service__subcmd__help__subcmd__install"
                ;;
            rift__cli__subcmd__service__subcmd__help,restart)
                cmd="rift__cli__subcmd__service__subcmd__help__subcmd__restart"
                ;;
            rift__cli__subcmd__service__subcmd__help,start)
                cmd="rift__cli__subcmd__service__subcmd__help__subcmd__start"
                ;;
            rift__cli__subcmd__service__subcmd__help,stop)
                cmd="rift__cli__subcmd__service__subcmd__help__subcmd__stop"
                ;;
            rift__cli__subcmd__service__subcmd__help,uninstall)
                cmd="rift__cli__subcmd__service__subcmd__help__subcmd__uninstall"
                ;;
            rift__cli__subcmd__subscribe,cli)
                cmd="rift__cli__subcmd__subscribe__subcmd__cli"
                ;;
            rift__cli__subcmd__subscribe,help)
                cmd="rift__cli__subcmd__subscribe__subcmd__help"
                ;;
            rift__cli__subcmd__subscribe,list-cli)
                cmd="rift__cli__subcmd__subscribe__subcmd__list__subcmd__cli"
                ;;
            rift__cli__subcmd__subscribe,mach)
                cmd="rift__cli__subcmd__subscribe__subcmd__mach"
                ;;
            rift__cli__subcmd__subscribe,unsub-cli)
                cmd="rift__cli__subcmd__subscribe__subcmd__unsub__subcmd__cli"
                ;;
            rift__cli__subcmd__subscribe,unsub-mach)
                cmd="rift__cli__subcmd__subscribe__subcmd__unsub__subcmd__mach"
                ;;
            rift__cli__subcmd__subscribe__subcmd__help,cli)
                cmd="rift__cli__subcmd__subscribe__subcmd__help__subcmd__cli"
                ;;
            rift__cli__subcmd__subscribe__subcmd__help,help)
                cmd="rift__cli__subcmd__subscribe__subcmd__help__subcmd__help"
                ;;
            rift__cli__subcmd__subscribe__subcmd__help,list-cli)
                cmd="rift__cli__subcmd__subscribe__subcmd__help__subcmd__list__subcmd__cli"
                ;;
            rift__cli__subcmd__subscribe__subcmd__help,mach)
                cmd="rift__cli__subcmd__subscribe__subcmd__help__subcmd__mach"
                ;;
            rift__cli__subcmd__subscribe__subcmd__help,unsub-cli)
                cmd="rift__cli__subcmd__subscribe__subcmd__help__subcmd__unsub__subcmd__cli"
                ;;
            rift__cli__subcmd__subscribe__subcmd__help,unsub-mach)
                cmd="rift__cli__subcmd__subscribe__subcmd__help__subcmd__unsub__subcmd__mach"
                ;;
            *)
                ;;
        esac
    done

    case "${cmd}" in
        rift__cli)
            opts="-h --help query execute subscribe service verify help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 1 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute)
            opts="-h --help window workspace layout config mission-control display save-and-exit debug serialize toggle-space-activated show-timing help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config)
            opts="-h --help set-animate set-animation-duration set-animation-fps set-animation-easing set-mouse-follows-focus set-mouse-hides-on-focus set-focus-follows-mouse set-stack-offset set-stack-default-orientation set-outer-gaps set-inner-gaps set-workspace-names set get save reload help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__get)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__help)
            opts="set-animate set-animation-duration set-animation-fps set-animation-easing set-mouse-follows-focus set-mouse-hides-on-focus set-focus-follows-mouse set-stack-offset set-stack-default-orientation set-outer-gaps set-inner-gaps set-workspace-names set get save reload help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__get)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__reload)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__save)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__animate)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__animation__subcmd__duration)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__animation__subcmd__easing)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__animation__subcmd__fps)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__focus__subcmd__follows__subcmd__mouse)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__inner__subcmd__gaps)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__mouse__subcmd__follows__subcmd__focus)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__mouse__subcmd__hides__subcmd__on__subcmd__focus)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__outer__subcmd__gaps)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__stack__subcmd__default__subcmd__orientation)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__stack__subcmd__offset)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__help__subcmd__set__subcmd__workspace__subcmd__names)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__reload)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__save)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__set)
            opts="-h --help <KEY> <VALUE>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__animate)
            opts="-h --help <VALUE>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__animation__subcmd__duration)
            opts="-h --help <VALUE>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__animation__subcmd__easing)
            opts="-h --help <VALUE>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__animation__subcmd__fps)
            opts="-h --help <VALUE>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__focus__subcmd__follows__subcmd__mouse)
            opts="-h --help true false"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__inner__subcmd__gaps)
            opts="-h --help <HORIZONTAL> <VERTICAL>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__mouse__subcmd__follows__subcmd__focus)
            opts="-h --help true false"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__mouse__subcmd__hides__subcmd__on__subcmd__focus)
            opts="-h --help true false"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__outer__subcmd__gaps)
            opts="-h --help <TOP> <LEFT> <BOTTOM> <RIGHT>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__stack__subcmd__default__subcmd__orientation)
            opts="-h --help <VALUE>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__stack__subcmd__offset)
            opts="-h --help <VALUE>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__config__subcmd__set__subcmd__workspace__subcmd__names)
            opts="-h --help [NAMES]..."
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__debug)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__display)
            opts="-h --help focus move-mouse-to-index move-mouse-to-uuid move-window help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__display__subcmd__focus)
            opts="-h --direction --index --uuid --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --direction)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --index)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --uuid)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__display__subcmd__help)
            opts="focus move-mouse-to-index move-mouse-to-uuid move-window help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__focus)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__move__subcmd__mouse__subcmd__to__subcmd__index)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__move__subcmd__mouse__subcmd__to__subcmd__uuid)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__display__subcmd__help__subcmd__move__subcmd__window)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__display__subcmd__move__subcmd__mouse__subcmd__to__subcmd__index)
            opts="-h --help <INDEX>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__display__subcmd__move__subcmd__mouse__subcmd__to__subcmd__uuid)
            opts="-h --help <UUID>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__display__subcmd__move__subcmd__window)
            opts="-h --direction --index --uuid --window-id --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --direction)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --index)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --uuid)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --window-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help)
            opts="window workspace layout config mission-control display save-and-exit debug serialize toggle-space-activated show-timing help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__config)
            opts="set-animate set-animation-duration set-animation-fps set-animation-easing set-mouse-follows-focus set-mouse-hides-on-focus set-focus-follows-mouse set-stack-offset set-stack-default-orientation set-outer-gaps set-inner-gaps set-workspace-names set get save reload"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__get)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__reload)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__save)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__animate)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__animation__subcmd__duration)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__animation__subcmd__easing)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__animation__subcmd__fps)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__focus__subcmd__follows__subcmd__mouse)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__inner__subcmd__gaps)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__mouse__subcmd__follows__subcmd__focus)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__mouse__subcmd__hides__subcmd__on__subcmd__focus)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__outer__subcmd__gaps)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__stack__subcmd__default__subcmd__orientation)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__stack__subcmd__offset)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__config__subcmd__set__subcmd__workspace__subcmd__names)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__debug)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__display)
            opts="focus move-mouse-to-index move-mouse-to-uuid move-window"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__display__subcmd__focus)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__display__subcmd__move__subcmd__mouse__subcmd__to__subcmd__index)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__display__subcmd__move__subcmd__mouse__subcmd__to__subcmd__uuid)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__display__subcmd__move__subcmd__window)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__layout)
            opts="ascend descend move-node join-window toggle-stack toggle-orientation unjoin toggle-focus-float adjust-master-ratio adjust-master-count promote-to-master swap-master-stack swap-windows scroll-strip snap-strip center-selection"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__adjust__subcmd__master__subcmd__count)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__adjust__subcmd__master__subcmd__ratio)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__ascend)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__center__subcmd__selection)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__descend)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__join__subcmd__window)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__move__subcmd__node)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__promote__subcmd__to__subcmd__master)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__scroll__subcmd__strip)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__snap__subcmd__strip)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__swap__subcmd__master__subcmd__stack)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__swap__subcmd__windows)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__toggle__subcmd__focus__subcmd__float)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__toggle__subcmd__orientation)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__toggle__subcmd__stack)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__layout__subcmd__unjoin)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__mission__subcmd__control)
            opts="show-all show-current dismiss"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__mission__subcmd__control__subcmd__dismiss)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__mission__subcmd__control__subcmd__show__subcmd__all)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__mission__subcmd__control__subcmd__show__subcmd__current)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__save__subcmd__and__subcmd__exit)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__serialize)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__show__subcmd__timing)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__toggle__subcmd__space__subcmd__activated)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__window)
            opts="next prev focus toggle-float toggle-fullscreen toggle-fullscreen-within-gaps resize-grow resize-shrink resize-by close add-scratchpad toggle-scratchpad"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__add__subcmd__scratchpad)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__close)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__focus)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__next)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__prev)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__resize__subcmd__by)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__resize__subcmd__grow)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__resize__subcmd__shrink)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__toggle__subcmd__float)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__toggle__subcmd__fullscreen)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__toggle__subcmd__fullscreen__subcmd__within__subcmd__gaps)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__window__subcmd__toggle__subcmd__scratchpad)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__workspace)
            opts="next prev switch move-window create last set-layout"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__create)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__last)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__move__subcmd__window)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__next)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__prev)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__set__subcmd__layout)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__help__subcmd__workspace__subcmd__switch)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout)
            opts="-h --help ascend descend move-node join-window toggle-stack toggle-orientation unjoin toggle-focus-float adjust-master-ratio adjust-master-count promote-to-master swap-master-stack swap-windows scroll-strip snap-strip center-selection help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__adjust__subcmd__master__subcmd__count)
            opts="-h --help <DELTA>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__adjust__subcmd__master__subcmd__ratio)
            opts="-h --help <DELTA>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__ascend)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__center__subcmd__selection)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__descend)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__help)
            opts="ascend descend move-node join-window toggle-stack toggle-orientation unjoin toggle-focus-float adjust-master-ratio adjust-master-count promote-to-master swap-master-stack swap-windows scroll-strip snap-strip center-selection help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__adjust__subcmd__master__subcmd__count)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__adjust__subcmd__master__subcmd__ratio)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__ascend)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__center__subcmd__selection)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__descend)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__join__subcmd__window)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__move__subcmd__node)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__promote__subcmd__to__subcmd__master)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__scroll__subcmd__strip)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__snap__subcmd__strip)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__swap__subcmd__master__subcmd__stack)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__swap__subcmd__windows)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__toggle__subcmd__focus__subcmd__float)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__toggle__subcmd__orientation)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__toggle__subcmd__stack)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__help__subcmd__unjoin)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__join__subcmd__window)
            opts="-h --help <DIRECTION>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__move__subcmd__node)
            opts="-h --help <DIRECTION>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__promote__subcmd__to__subcmd__master)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__scroll__subcmd__strip)
            opts="-h --help <DELTA>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__snap__subcmd__strip)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__swap__subcmd__master__subcmd__stack)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__swap__subcmd__windows)
            opts="-h --help <A> <B>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__toggle__subcmd__focus__subcmd__float)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__toggle__subcmd__orientation)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__toggle__subcmd__stack)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__layout__subcmd__unjoin)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__mission__subcmd__control)
            opts="-h --help show-all show-current dismiss help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__dismiss)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__help)
            opts="show-all show-current dismiss help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__help__subcmd__dismiss)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__help__subcmd__show__subcmd__all)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__help__subcmd__show__subcmd__current)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__show__subcmd__all)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__mission__subcmd__control__subcmd__show__subcmd__current)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__save__subcmd__and__subcmd__exit)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__serialize)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__show__subcmd__timing)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__toggle__subcmd__space__subcmd__activated)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window)
            opts="-h --help next prev focus toggle-float toggle-fullscreen toggle-fullscreen-within-gaps resize-grow resize-shrink resize-by close add-scratchpad toggle-scratchpad help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__add__subcmd__scratchpad)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__close)
            opts="-h --window-id --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --window-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__focus)
            opts="-h --help <DIRECTION>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__help)
            opts="next prev focus toggle-float toggle-fullscreen toggle-fullscreen-within-gaps resize-grow resize-shrink resize-by close add-scratchpad toggle-scratchpad help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__add__subcmd__scratchpad)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__close)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__focus)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__next)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__prev)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__resize__subcmd__by)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__resize__subcmd__grow)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__resize__subcmd__shrink)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__toggle__subcmd__float)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__toggle__subcmd__fullscreen)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__toggle__subcmd__fullscreen__subcmd__within__subcmd__gaps)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__help__subcmd__toggle__subcmd__scratchpad)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__next)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__prev)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__resize__subcmd__by)
            opts="-h --help <AMOUNT>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__resize__subcmd__grow)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__resize__subcmd__shrink)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__toggle__subcmd__float)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__toggle__subcmd__fullscreen)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__toggle__subcmd__fullscreen__subcmd__within__subcmd__gaps)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__window__subcmd__toggle__subcmd__scratchpad)
            opts="-h --name --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --name)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__workspace)
            opts="-h --help next prev switch move-window create last set-layout help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__workspace__subcmd__create)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__workspace__subcmd__help)
            opts="next prev switch move-window create last set-layout help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__create)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__last)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__move__subcmd__window)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__next)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__prev)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__set__subcmd__layout)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__workspace__subcmd__help__subcmd__switch)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__workspace__subcmd__last)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__workspace__subcmd__move__subcmd__window)
            opts="-h --help <WORKSPACE_ID> [WINDOW_ID]"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__workspace__subcmd__next)
            opts="-h --help true false"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__workspace__subcmd__prev)
            opts="-h --help true false"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__workspace__subcmd__set__subcmd__layout)
            opts="-h --workspace-id --help <MODE>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --workspace-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__execute__subcmd__workspace__subcmd__switch)
            opts="-h --help <WORKSPACE_ID>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help)
            opts="query execute subscribe service verify help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute)
            opts="window workspace layout config mission-control display save-and-exit debug serialize toggle-space-activated show-timing"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__config)
            opts="set-animate set-animation-duration set-animation-fps set-animation-easing set-mouse-follows-focus set-mouse-hides-on-focus set-focus-follows-mouse set-stack-offset set-stack-default-orientation set-outer-gaps set-inner-gaps set-workspace-names set get save reload"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__get)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__reload)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__save)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__animate)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__animation__subcmd__duration)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__animation__subcmd__easing)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__animation__subcmd__fps)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__focus__subcmd__follows__subcmd__mouse)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__inner__subcmd__gaps)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__mouse__subcmd__follows__subcmd__focus)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__mouse__subcmd__hides__subcmd__on__subcmd__focus)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__outer__subcmd__gaps)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__stack__subcmd__default__subcmd__orientation)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__stack__subcmd__offset)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__config__subcmd__set__subcmd__workspace__subcmd__names)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__debug)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__display)
            opts="focus move-mouse-to-index move-mouse-to-uuid move-window"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__display__subcmd__focus)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__display__subcmd__move__subcmd__mouse__subcmd__to__subcmd__index)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__display__subcmd__move__subcmd__mouse__subcmd__to__subcmd__uuid)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__display__subcmd__move__subcmd__window)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__layout)
            opts="ascend descend move-node join-window toggle-stack toggle-orientation unjoin toggle-focus-float adjust-master-ratio adjust-master-count promote-to-master swap-master-stack swap-windows scroll-strip snap-strip center-selection"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__adjust__subcmd__master__subcmd__count)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__adjust__subcmd__master__subcmd__ratio)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__ascend)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__center__subcmd__selection)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__descend)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__join__subcmd__window)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__move__subcmd__node)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__promote__subcmd__to__subcmd__master)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__scroll__subcmd__strip)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__snap__subcmd__strip)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__swap__subcmd__master__subcmd__stack)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__swap__subcmd__windows)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__toggle__subcmd__focus__subcmd__float)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__toggle__subcmd__orientation)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__toggle__subcmd__stack)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__layout__subcmd__unjoin)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__mission__subcmd__control)
            opts="show-all show-current dismiss"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__mission__subcmd__control__subcmd__dismiss)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__mission__subcmd__control__subcmd__show__subcmd__all)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__mission__subcmd__control__subcmd__show__subcmd__current)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__save__subcmd__and__subcmd__exit)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__serialize)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__show__subcmd__timing)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__toggle__subcmd__space__subcmd__activated)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__window)
            opts="next prev focus toggle-float toggle-fullscreen toggle-fullscreen-within-gaps resize-grow resize-shrink resize-by close add-scratchpad toggle-scratchpad"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__add__subcmd__scratchpad)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__close)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__focus)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__next)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__prev)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__resize__subcmd__by)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__resize__subcmd__grow)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__resize__subcmd__shrink)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__toggle__subcmd__float)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__toggle__subcmd__fullscreen)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__toggle__subcmd__fullscreen__subcmd__within__subcmd__gaps)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__window__subcmd__toggle__subcmd__scratchpad)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__workspace)
            opts="next prev switch move-window create last set-layout"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__create)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__last)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__move__subcmd__window)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__next)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__prev)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__set__subcmd__layout)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__execute__subcmd__workspace__subcmd__switch)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 5 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__query)
            opts="workspaces windows displays window applications layout workspace-layout metrics"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__query__subcmd__applications)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__query__subcmd__displays)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__query__subcmd__layout)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__query__subcmd__metrics)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__query__subcmd__window)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__query__subcmd__windows)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__query__subcmd__workspace__subcmd__layout)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__query__subcmd__workspaces)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__service)
            opts="install uninstall start stop restart"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__service__subcmd__install)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__service__subcmd__restart)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__service__subcmd__start)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__service__subcmd__stop)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__service__subcmd__uninstall)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__subscribe)
            opts="mach cli unsub-mach unsub-cli list-cli"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__subscribe__subcmd__cli)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__subscribe__subcmd__list__subcmd__cli)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__subscribe__subcmd__mach)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__subscribe__subcmd__unsub__subcmd__cli)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__subscribe__subcmd__unsub__subcmd__mach)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__help__subcmd__verify)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query)
            opts="-h --help workspaces windows displays window applications layout workspace-layout metrics help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query__subcmd__applications)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query__subcmd__displays)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query__subcmd__help)
            opts="workspaces windows displays window applications layout workspace-layout metrics help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query__subcmd__help__subcmd__applications)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query__subcmd__help__subcmd__displays)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query__subcmd__help__subcmd__layout)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query__subcmd__help__subcmd__metrics)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query__subcmd__help__subcmd__window)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query__subcmd__help__subcmd__windows)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query__subcmd__help__subcmd__workspace__subcmd__layout)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query__subcmd__help__subcmd__workspaces)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query__subcmd__layout)
            opts="-h --help <SPACE_ID>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query__subcmd__metrics)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query__subcmd__window)
            opts="-h --help <WINDOW_ID>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query__subcmd__windows)
            opts="-h --space-id --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --space-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query__subcmd__workspace__subcmd__layout)
            opts="-h --space-id --workspace-id --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --space-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --workspace-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__query__subcmd__workspaces)
            opts="-h --space-id --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --space-id)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__service)
            opts="-h --help install uninstall start stop restart help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__service__subcmd__help)
            opts="install uninstall start stop restart help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__service__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__service__subcmd__help__subcmd__install)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__service__subcmd__help__subcmd__restart)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__service__subcmd__help__subcmd__start)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__service__subcmd__help__subcmd__stop)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__service__subcmd__help__subcmd__uninstall)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__service__subcmd__install)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__service__subcmd__restart)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__service__subcmd__start)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__service__subcmd__stop)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__service__subcmd__uninstall)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__subscribe)
            opts="-h --help mach cli unsub-mach unsub-cli list-cli help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__subscribe__subcmd__cli)
            opts="-h --event --command --args --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --event)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --command)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --args)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__subscribe__subcmd__help)
            opts="mach cli unsub-mach unsub-cli list-cli help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__subscribe__subcmd__help__subcmd__cli)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__subscribe__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__subscribe__subcmd__help__subcmd__list__subcmd__cli)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__subscribe__subcmd__help__subcmd__mach)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__subscribe__subcmd__help__subcmd__unsub__subcmd__cli)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__subscribe__subcmd__help__subcmd__unsub__subcmd__mach)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__subscribe__subcmd__list__subcmd__cli)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__subscribe__subcmd__mach)
            opts="-h --help <EVENT>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__subscribe__subcmd__unsub__subcmd__cli)
            opts="-h --help <EVENT>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__subscribe__subcmd__unsub__subcmd__mach)
            opts="-h --help <EVENT>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        rift__subcmd__cli__subcmd__verify)
            opts="-h --help <CONFIG_PATH>"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
    esac
}

if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -F _rift-cli -o nosort -o bashdefault -o default rift-cli
else
    complete -F _rift-cli -o bashdefault -o default rift-cli
fi
