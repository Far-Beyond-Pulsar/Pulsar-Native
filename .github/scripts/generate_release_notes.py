#!/usr/bin/env python3
"""
Generate structured release notes using GitHub Models API.
Uses official GitHub Models API with PAT authentication.
"""

import requests
import json
import argparse
import sys

def get_args():
    """Parse and return command-line arguments."""
    parser = argparse.ArgumentParser(description="Generate structured release notes with GitHub Models API.")
    parser.add_argument("--github-token", required=True, help="GitHub PAT with models:read permission.")
    parser.add_argument("--repo-owner", required=True, help="Repository owner.")
    parser.add_argument("--repo-name", required=True, help="Repository name.")
    parser.add_argument("--version", required=True, help="Current version being released.")
    parser.add_argument("--commits", required=True, help="JSON array of commits with sha, message, and author.")
    parser.add_argument("--model", default="gpt-4o", help="Model to use (default: gpt-4o).")
    return parser.parse_args()

def build_prompt(repo_owner, repo_name, version, commits_json):
    """Build the prompt for generating release notes."""
    commits = json.loads(commits_json)
    
    if not commits:
        return None
    
    commits_text = "\n".join([
        f"- {commit['message']} (by {commit['author']})"
        for commit in commits
    ])
    
    return f"""Generate structured release notes for version {version} of {repo_owner}/{repo_name}.

Commits since last release:
{commits_text}

Please analyze these commits and categorize the changes into sections. Use these sections in this order and include the emoji in the H2 heading:
- **✨ Highlights**: 2 to 5 most important user-facing changes
- **⚠️ Breaking Changes**: Any breaking changes (see special rules below)
- **🚀 Features**: New functionality or capabilities
- **🛠️ Improvements**: Enhancements to existing features
- **🐛 Bug Fixes**: Corrections to defects
- **⚡ Performance**: Speed, memory, or efficiency improvements
- **🔒 Security**: Vulnerability fixes or security hardening
- **🧩 Plugin API**: Changes that affect plugin authors, even if not breaking
- **📚 Documentation**: Documentation updates
- **🧰 Developer Experience**: Tooling, build system, CI, scripts, tests, or internal workflows
- **🧹 Internal**: Refactoring, dependencies, code cleanup

Breaking changes rules:
- If any change is a breaking change to the plugins API, include a GitHub CAUTION callout under the **Breaking Changes** section.
- Use the exact format:
> [!CAUTION]
> Breaking change: <short description>
- If there are multiple breaking changes, include multiple callouts.

Formatting rules (strict):
- Use GitHub Markdown with H2 headings (e.g. "## Features").
- One blank line between sections, and one blank line between a heading and its list.
- Use '-' bullet points only, no nested lists.
- Each bullet: short sentence, sentence case, ends with a period.
- Each bullet MUST include GitHub mentions for everyone involved in the commit (author and any co-authors).
- Use @username mentions when available; otherwise include the provided name as plain text.
- No trailing whitespace.

General guidelines:
- If a section has no items, omit that section entirely.
- Merge similar commits into single entries when appropriate.
- Focus on user-facing changes for Highlights, Features, Improvements, Bug Fixes.
- Place technical/infrastructure changes in Developer Experience or Internal.

Return ONLY the markdown content without code fences or explanations/anecdotes."""

def generate_release_notes(github_token, model, prompt):
    """Call GitHub Models API to generate release notes."""
    headers = {
        "Authorization": f"Bearer {github_token}",
        "Accept": "application/vnd.github+json",
        "Content-Type": "application/json"
    }
    
    data = {
        "model": model,
        "messages": [
            {"role": "user", "content": prompt}
        ],
        "temperature": 0.3,
        "max_tokens": 2000
    }
    
    response = requests.post(
        "https://models.github.ai/inference/chat/completions",
        headers=headers,
        json=data,
        timeout=30
    )
    
    if response.status_code != 200:
        raise Exception(f"GitHub Models API request failed. Status: {response.status_code}, Response: {response.text}")
    
    result = response.json()
    return result['choices'][0]['message']['content'].strip()

def main():
    args = get_args()
    
    try:
        prompt = build_prompt(
            repo_owner=args.repo_owner,
            repo_name=args.repo_name,
            version=args.version,
            commits_json=args.commits
        )
        
        if not prompt:
            print("## What's Changed\n\nNo commits found since last release.", file=sys.stderr)
            sys.exit(1)
        
        release_notes = generate_release_notes(
            github_token=args.github_token,
            model=args.model,
            prompt=prompt
        )
        
        # Output the release notes
        print(release_notes)
        
    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)

if __name__ == "__main__":
    main()
