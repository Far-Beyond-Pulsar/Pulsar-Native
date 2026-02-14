# GitHub Actions Scripts

## generate_release_notes.py

Generates structured release notes using GitHub Models API based on commit history.

### How It Works

The script uses GitHub's official Models API to analyze commit messages and generate categorized release notes. It automatically:
- Categorizes changes into Features, Improvements, Bug Fixes, Documentation, and Internal
- Merges similar commits into concise entries
- Focuses on user-facing changes
- Outputs clean markdown for GitHub releases

### Authentication

The script requires a Personal Access Token (PAT) with GitHub Copilot access. Set up:

1. Go to GitHub Settings → Developer Settings → Personal Access Tokens (Fine-grained)
2. Create a token with the `models` scope (read permission)
3. Add it as a repository secret named `CI_COPILOT_TOKEN` at:
   `Settings → Secrets and variables → Actions → New repository secret`

**Note:** Your GitHub account must have an active Copilot subscription (Pro, Pro+, Business, or Enterprise) to use the Models API.

### Model Selection

By default, the script uses `gpt-4o`. You can change the model by adding `--model` parameter:
- `gpt-4o` (default) - Best quality
- `gpt-4o-mini` - Faster, lower cost
- Other available models at https://models.github.ai/catalog/models

### Fallback Behavior

If the GitHub Models API request fails (rate limits, network issues, etc.), the workflow automatically falls back to generating basic release notes from commit messages without AI categorization.

### Dependencies

- Python 3.11+
- requests

These are automatically installed by the workflow.
