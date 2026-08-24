Register-ArgumentCompleter -Native -CommandName modeltap -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)

    $commandElements = $commandAst.CommandElements
    $count = $commandElements.Count

    if ($count -le 1 -or ($count -eq 2 -and $cursorPosition -le $commandElements[1].Extent.EndOffset)) {
        $subcommands = @('run', 'validate', 'ca-init', 'help', '--help', '-h', '--version', '-V')
        $subcommands | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
            [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
        }
        return
    }

    $subcommand = $commandElements[1].Extent.Text

    switch ($subcommand) {
        'run' {
            $options = @('-c', '--config', '-h', '--help')
            $options | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
                [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterName', $_)
            }
        }
        'validate' {
            $options = @('-c', '--config', '-h', '--help')
            $options | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
                [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterName', $_)
            }
        }
        'ca-init' {
            $options = @('--cert', '--key', '-h', '--help')
            $options | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
                [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterName', $_)
            }
        }
    }
}
