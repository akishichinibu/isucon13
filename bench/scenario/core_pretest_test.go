package scenario

import (
	"context"
	"testing"

	"github.com/isucon/isucandar/agent"
	"github.com/isucon/isucon13/bench/isupipe"
	"github.com/stretchr/testify/assert"
)

func TestPretest(t *testing.T) {
	ctx := context.Background()

	client, err := isupipe.NewClient(
		agent.WithBaseURL(webappIPAddress),
	)
	if err != nil {
		t.Fatal(err)
	}

	err = Pretest(ctx, client)
	assert.NoError(t, err)
}