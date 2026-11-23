package launch

import (
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/libraries/commandargs"
)

func shellCommandString(executable string, arguments []string) string {
	var command strings.Builder
	command.WriteString(commandargs.QuoteArgument(executable))
	for _, argument := range arguments {
		command.WriteByte(' ')
		command.WriteString(commandargs.QuoteArgument(argument))
	}
	return command.String()
}
