_modeltap() {
  local current previous command
  COMPREPLY=()
  current="${COMP_WORDS[COMP_CWORD]}"
  previous="${COMP_WORDS[COMP_CWORD - 1]}"
  command="${COMP_WORDS[1]}"

  case "${previous}" in
    -c|--config)
      COMPREPLY=( $(compgen -f -- "${current}") )
      return 0
      ;;
    --cert|--key)
      COMPREPLY=( $(compgen -f -- "${current}") )
      return 0
      ;;
  esac

  if [[ ${COMP_CWORD} -eq 1 ]]; then
    COMPREPLY=( $(compgen -W 'run validate ca-init help --help --version -h -V' -- "${current}") )
    return 0
  fi

  case "${command}" in
    run)
      COMPREPLY=( $(compgen -W '-c --config -h --help' -- "${current}") )
      ;;
    validate)
      COMPREPLY=( $(compgen -W '-c --config -h --help' -- "${current}") )
      ;;
    ca-init)
      COMPREPLY=( $(compgen -W '--cert --key -h --help' -- "${current}") )
      ;;
  esac
}

complete -F _modeltap modeltap
