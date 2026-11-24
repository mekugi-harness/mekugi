package options

import (
	"reflect"
	"testing"
)

func TestProfileSelectorFormsOperandsAndRejections(t *testing.T) {
	for _, selector := range [][]string{{"-p", "work"}, {"--profile", "work"}, {"-pwork"}, {"-p=work"}, {"--profile=work"}} {
		input := append([]string{"exec", "--model", "-poperand", "-c", "--profile=operand"}, selector...)
		input = append(input, "--", "-p", "literal")
		name, args, err := ProfileSelector(input)
		want := []string{"exec", "--model", "-poperand", "-c", "--profile=operand", "--", "-p", "literal"}
		if err != nil || name != "work" || !reflect.DeepEqual(args, want) {
			t.Fatalf("%q: %q %q %v", input, name, args, err)
		}
	}
	for _, input := range [][]string{{"-p"}, {"-p", ""}, {"--profile="}, {"-p../x"}, {"-pwork.toml"}, {"-pwork", "-pwork"}, {"-p工作"}} {
		if _, _, err := ProfileSelector(input); err == nil {
			t.Fatalf("accepted invalid selector: %q", input)
		}
	}
}
