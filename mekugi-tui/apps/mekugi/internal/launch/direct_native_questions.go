package launch

import (
	"context"
	"encoding/json/jsontext"
	json "encoding/json/v2"
	"errors"
	"fmt"
	"net/http"
	"slices"
	"strings"

	"github.com/mekugi-harness/mekugi/mekugi-tui/apps/mekugi/internal/sessionapi"
	"github.com/mekugi-harness/mekugi/mekugi-tui/backends/codex/binding"
	"github.com/mekugi-harness/mekugi/mekugi-tui/modules/conversation/prompt"
)

const (
	asyncQuestionTool  = "request_user_input_async"
	questionReplyStart = "<send_user_message_question_reply>"
	questionReplyEnd   = "</send_user_message_question_reply>"
)

type nativeQuestionReply struct {
	ID       string `json:"questionItemId"`
	Question string `json:"question"`
	Answer   string `json:"answer"`
}

func parseNativeQuestionReplies(text string) []nativeQuestionReply {
	text = strings.TrimSpace(text)
	if strings.HasPrefix(text, "# Context from my IDE setup:\n") {
		at := strings.LastIndex(text, "\n## My request for Codex:\n")
		if at < 0 {
			return nil
		}
		text = strings.TrimSpace(text[at+len("\n## My request for Codex:\n"):])
	}
	payload, ok := strings.CutPrefix(text, questionReplyStart)
	if !ok {
		return nil
	}
	payload, ok = strings.CutSuffix(payload, questionReplyEnd)
	if !ok {
		return nil
	}
	var replies []nativeQuestionReply
	if json.Unmarshal([]byte(payload), &replies) != nil {
		var reply nativeQuestionReply
		if json.Unmarshal([]byte(payload), &reply) == nil {
			replies = []nativeQuestionReply{reply}
		}
	}
	return replies
}

func (c *directNativeConnection) observeAsyncQuestionsLocked(thread, turn, item string, raw jsontext.Value) {
	var incoming []struct {
		Header   string `json:"header"`
		Question string `json:"question"`
		Options  []struct {
			Label       string `json:"label"`
			Description string `json:"description"`
			Preview     string `json:"preview"`
			Recommended bool   `json:"recommended"`
		} `json:"options"`
	}
	if item == "" || json.Unmarshal(raw, &incoming) != nil || len(incoming) == 0 {
		return
	}
	key := asyncQuestionTool + ":" + thread + ":" + item
	if c.requests[key] != nil {
		return
	}
	call := &prompt.CallView{ID: key, Thread: thread, Turn: turn, Item: item}
	for i, source := range incoming {
		// Match stock Codex and desktop's stable identity for each question.
		id, _ := json.Marshal([]any{asyncQuestionTool, item, i})
		question := prompt.QuestionView{ID: string(id), Header: source.Header, Text: source.Question, Pending: true}
		for _, option := range source.Options[:min(32, len(source.Options))] {
			if len(option.Label) <= 512 {
				question.Choices = append(question.Choices, prompt.Option(option))
			}
		}
		call.Questions = append(call.Questions, question)
	}
	c.requests[key] = &directPendingServerRequest{threadID: thread, turnID: turn, itemID: item, question: call}
	c.requestOrder = append(c.requestOrder, key)
}

func (c *directNativeConnection) observeQuestionReplyLocked(thread, text string) string {
	replies := parseNativeQuestionReplies(text)
	if len(replies) == 0 {
		return text
	}
	display := make([]string, 0, len(replies))
	for _, reply := range replies {
		display = append(display, "> "+reply.Question+"\n\n"+reply.Answer)
		c.requestOrder = slices.DeleteFunc(c.requestOrder, func(key string) bool {
			req := c.requests[key]
			if req == nil || req.threadID != thread || req.question == nil || req.question.Sync {
				return false
			}
			if reply.ID != req.itemID {
				questions := slices.Clone(req.question.Questions)
				index := slices.IndexFunc(questions, func(q prompt.QuestionView) bool { return q.ID == reply.ID })
				if index < 0 {
					return false
				}
				questions[index].Pending = false
				questions[index].State = "answered"
				questions[index].Answer = []string{reply.Answer}
				if slices.ContainsFunc(questions, func(q prompt.QuestionView) bool { return q.Pending }) {
					req.question.Questions = questions
					return false
				}
			}
			delete(c.requests, key)
			return true
		})
	}
	return strings.Join(display, "\n\n")
}

// Called with the connection lock held. Async answers are regular native user
// input, not a response to an app-server JSON-RPC request.
func (c *directNativeConnection) answerAsyncQuestions(ctx context.Context, in sessionapi.NativeAnswers, req *directPendingServerRequest) error {
	if req.question.Sent {
		c.mu.Unlock()
		return errors.New("question answers are already settling")
	}
	replies := make([]nativeQuestionReply, 0, len(in.Answers))
	for _, answer := range in.Answers {
		index := slices.IndexFunc(req.question.Questions, func(q prompt.QuestionView) bool { return q.ID == answer.Question })
		if index < 0 || !req.question.Questions[index].Pending {
			c.mu.Unlock()
			return prompt.ErrQuestionUnavailable
		}
		if answer.Skip {
			continue
		}
		question := req.question.Questions[index]
		if len(answer.Draft.Images) > 0 || len(answer.Draft.AnswerImages) > 0 {
			c.mu.Unlock()
			return &sessionapi.NativeFailure{Status: http.StatusBadRequest, Reason: "Async question replies accept text only."}
		}
		text := strings.TrimSpace(answer.Draft.Text)
		if answer.Choice >= 0 && answer.Choice < len(question.Choices) {
			text = question.Choices[answer.Choice].Label
			if note := strings.TrimSpace(answer.Note); note != "" {
				text += "\n" + note
			}
		}
		if text == "" {
			c.mu.Unlock()
			return prompt.ErrEmptyAnswer
		}
		label := strings.ToValidUTF8(question.Text[:min(512, len(question.Text))], "")
		label = strings.NewReplacer("\n", " ", "\r", " ").Replace(label)
		replies = append(replies, nativeQuestionReply{ID: answer.Question, Question: label, Answer: text})
	}
	thread := req.threadID
	turn := ""
	if thread == c.mainThread || thread == c.sideThread {
		turn = c.activeTurnForLocked(thread)
	}
	req.question.Sent = true
	c.mu.Unlock()

	var err error
	if len(replies) > 0 {
		payload, marshalErr := json.Marshal(replies)
		if marshalErr != nil {
			err = fmt.Errorf("encode question answers: %w", marshalErr)
		} else {
			text := questionReplyStart + "\n" + string(payload) + "\n" + questionReplyEnd
			input := []map[string]any{binding.TextInput(text, nil)}
			if turn != "" {
				err = c.wire.Call(ctx, "turn/steer", map[string]any{
					"threadId": thread, "input": input, "expectedTurnId": turn,
				}, nil)
			} else {
				err = c.startInputTurn(ctx, thread, input)
			}
		}
	}
	c.mu.Lock()
	if c.requests[in.Call] == req {
		if err != nil {
			req.question.Sent = false
		} else {
			delete(c.requests, in.Call)
			c.requestOrder = slices.DeleteFunc(c.requestOrder, func(key string) bool { return key == in.Call })
		}
		c.publishLocked()
	}
	c.mu.Unlock()
	return err
}
