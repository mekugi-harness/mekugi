package presentation

import (
	"slices"
)

// Source: internal/router/live_activity_skills.go:12:133@[ebde9ad5].
type SkillSet []string

func (s SkillSet) With(names ...string) SkillSet {
	for _, name := range names {
		if !slices.Contains(s, name) {
			s = append(s, name)
		}
	}
	return s
}
