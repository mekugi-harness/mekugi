package changes

import "github.com/mekugi-harness/mekugi/mekugi-tui/contracts/command"

var Command = command.Definition[Service]{Name: "mchanges", Run: Service.Execute}
