package nativeevidence

import (
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"testing"

	"gopkg.in/yaml.v3"
)

var pinnedActionPattern = regexp.MustCompile(`^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+@[0-9a-f]{40}$`)

// TestNativeEvidenceWorkflowKeepsSecurityAndOwnershipBoundaries 覆盖 native
// evidence stage。该 stage 现在由 platform-smoke.yml 承载（native_evidence job）：
// 它仍然只由手动 dispatch 运行，但整份 workflow 还包含 PR smoke worker，因此这里
// 既检查 evidence 自有的边界，也检查两个 stage 的互斥。
func TestNativeEvidenceWorkflowKeepsSecurityAndOwnershipBoundaries(t *testing.T) {
	t.Parallel()

	path := filepath.Join(findRepositoryRoot(t), ".github", "workflows", "platform-smoke.yml")
	body, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	workflow := string(body)
	for _, required := range []string{
		"workflow_dispatch:",
		"./tools/platformmatrix --capability native-evidence",
		"scripts/build-platform.sh",
		"--cc '${{ matrix.cc }}'",
		"CC='${{ matrix.cc }}' go test ./internal/media/ugoira",
		"scripts/cmd/nativeevidence record",
		"refs/heads/main",
		// evidence dispatch 不得顺带执行六平台 PR smoke，反之亦然。
		"if: ${{ inputs.evidence }}",
		"if: ${{ !inputs.evidence }}",
	} {
		if !strings.Contains(workflow, required) {
			t.Fatalf("native evidence workflow missing ownership/security contract %q", required)
		}
	}
	// 禁止项只针对 evidence stage：workflow 级的校验另见 testWorkflowStageBoundaries。
	for _, forbidden := range []string{
		"build-staticlibs.sh",
		"go build -trimpath",
		"releaseassets package",
		"gh release",
		"git push",
		"docker push",
	} {
		if strings.Contains(workflow, forbidden) {
			t.Fatalf("native evidence workflow owns forbidden duplicate/side-effect %q", forbidden)
		}
	}

	var document yaml.Node
	if err := yaml.Unmarshal(body, &document); err != nil {
		t.Fatalf("parse native evidence workflow: %v", err)
	}
	if document.Kind != yaml.DocumentNode || len(document.Content) != 1 || document.Content[0].Kind != yaml.MappingNode {
		t.Fatal("native evidence workflow must contain one mapping document")
	}
	root := document.Content[0]
	if workflowHasAmbiguousYAML(root) {
		t.Fatal("native evidence workflow must not contain aliases, merge keys, or duplicate mapping keys")
	}
	permissions := workflowMappingValue(t, root, "permissions")
	if permissions.Kind != yaml.MappingNode || len(permissions.Content) != 0 {
		t.Fatal("native evidence workflow must keep empty global permissions")
	}
	if workflowContainsSecretReference(root) {
		t.Fatal("native evidence workflow must not reference GitHub secrets")
	}

	jobs := workflowMappingValue(t, root, "jobs")
	if jobs.Kind != yaml.MappingNode {
		t.Fatal("native evidence jobs must be a mapping")
	}
	// evidence stage 的两个 job 才承担 evidence 的权限边界；整个 workflow 的
	// job 集合包含只读的 PR smoke worker 与持有 checks: write 的 result bridge。
	for _, jobName := range []string{"setup", "native_evidence"} {
		job, ok := workflowOptionalMappingValue(jobs, jobName)
		if !ok {
			t.Fatalf("native evidence workflow missing job %q", jobName)
		}
		permissions := workflowMappingValue(t, job, "permissions")
		if permissions.Kind != yaml.MappingNode || len(permissions.Content) != 2 ||
			permissions.Content[0].Value != "contents" || permissions.Content[1].Value != "read" {
			t.Fatalf("native evidence job %q must have only contents: read", jobName)
		}
	}

	for index := 0; index+1 < len(jobs.Content); index += 2 {
		jobName := jobs.Content[index].Value
		job := jobs.Content[index+1]
		if jobName == "publish" {
			// publish 是 PR smoke 的 Check Run bridge；evidence run 不携带
			// check_run_id，因此它不会运行，也不写任何 evidence 数据。
			continue
		}
		walkWorkflowMappings(job, func(mapping *yaml.Node) {
			if _, ok := workflowOptionalMappingValue(mapping, "environment"); ok {
				t.Errorf("native evidence job %q must not use a GitHub environment", jobName)
			}
			uses, ok := workflowOptionalMappingValue(mapping, "uses")
			if !ok {
				return
			}
			if uses.Kind != yaml.ScalarNode {
				t.Errorf("native evidence action reference in %q must be a scalar", jobName)
				return
			}
			if !pinnedActionPattern.MatchString(uses.Value) {
				t.Errorf("GitHub action must be pinned to a full commit SHA: %s", uses.Value)
			}
			if !strings.HasPrefix(uses.Value, "actions/checkout@") {
				return
			}
			with := workflowMappingValue(t, mapping, "with")
			persist := workflowMappingValue(t, with, "persist-credentials")
			if persist.Kind != yaml.ScalarNode || persist.Value != "false" {
				t.Errorf("native evidence checkout in %q must disable persisted credentials", jobName)
			}
		})
	}
}

func workflowMappingValue(t *testing.T, mapping *yaml.Node, key string) *yaml.Node {
	t.Helper()
	value, ok := workflowOptionalMappingValue(mapping, key)
	if !ok {
		t.Fatalf("native evidence workflow missing mapping key %q", key)
	}
	return value
}

func workflowOptionalMappingValue(mapping *yaml.Node, key string) (*yaml.Node, bool) {
	if mapping == nil || mapping.Kind != yaml.MappingNode {
		return nil, false
	}
	for index := 0; index+1 < len(mapping.Content); index += 2 {
		if mapping.Content[index].Value == key {
			return mapping.Content[index+1], true
		}
	}
	return nil, false
}

func walkWorkflowMappings(node *yaml.Node, visit func(*yaml.Node)) {
	if node == nil {
		return
	}
	if node.Kind == yaml.MappingNode {
		visit(node)
	}
	for _, child := range node.Content {
		walkWorkflowMappings(child, visit)
	}
}

func workflowHasAmbiguousYAML(node *yaml.Node) bool {
	if node == nil {
		return false
	}
	if node.Kind == yaml.AliasNode {
		return true
	}
	if node.Kind == yaml.MappingNode {
		seen := make(map[string]struct{}, len(node.Content)/2)
		for index := 0; index+1 < len(node.Content); index += 2 {
			key := node.Content[index]
			if key.Kind != yaml.ScalarNode || key.Value == "<<" {
				return true
			}
			if _, duplicate := seen[key.Value]; duplicate {
				return true
			}
			seen[key.Value] = struct{}{}
		}
	}
	for _, child := range node.Content {
		if workflowHasAmbiguousYAML(child) {
			return true
		}
	}
	return false
}

func workflowContainsSecretReference(node *yaml.Node) bool {
	if node == nil {
		return false
	}
	if node.Kind == yaml.ScalarNode {
		lowered := strings.ToLower(node.Value)
		for _, marker := range []string{"secrets.", "secrets[", "secrets ["} {
			if strings.Contains(lowered, marker) {
				return true
			}
		}
	}
	for _, child := range node.Content {
		if workflowContainsSecretReference(child) {
			return true
		}
	}
	return false
}
