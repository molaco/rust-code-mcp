Prompt for Analyzing a Large Codebase:

I need you to analyze the Rust codebase at /home/molaco/Documents/burn/ . Please provide:

1. **Health Check**: Verify the MCP system is working properly for this directory
2. **Codebase Overview**:
   - Total number of files and overall structure
   - Main modules and their purposes
   - Key entry points (main.rs, lib.rs)
3. **Complexity Analysis**: Identify the most complex files (high cyclomatic complexity, LOC)
4. **Dependency Map**: Show the major dependency relationships between modules
5. **Core Functionality**: Find and explain the main functions/structs that drive the application
6. **Call Graphs**: For the 3-5 most important functions, show their call graphs
7. **Code Patterns**: Search for common patterns (error handling, async usage, trait implementations)
8. **Potential Issues**: Look for:
   - Highly complex functions that might need refactoring
   - Circular dependencies
   - Dead code or unused imports
   - Heavy coupling between modules

Use the rust-code-mcp tools systematically to gather this information. Start with health_check, then use search, find_definition, analyze_complexity, get_dependencies, and get_call_graph as needed.

Do we need to index first? do the tools do it auto?



____


Prompt for Generating Agent Context:

I need you to generate comprehensive context about the Rust codebase at [PATH_TO_CODEBASE] for my AI agent to work on [SPECIFIC_TASK/FEATURE].

Please gather and format the following information:

1. **Relevant Code Sections**:
   - Search for code related to: [KEYWORDS/FEATURES]
   - Find definitions of key types/functions: [SPECIFIC_SYMBOLS]
   - Get similar code examples for: [DESCRIPTION_OF_PATTERN]

2. **Dependencies & Relationships**:
   - Show what imports/uses the relevant modules
   - Find all references to the main structs/functions involved
   - Generate call graphs for critical functions

3. **Code Examples**:
   - Extract working examples of similar functionality
   - Show how existing patterns are implemented
   - Include test cases if available

4. **Context Summary** - Format as:
Overview

   [Brief description of relevant codebase area]

Key Files

- file_path:line - description

Important Types/Functions

- Symbol name: purpose and location

Code Patterns Used

   [Relevant patterns from codebase]

Dependencies

   [What this code depends on and what depends on it]

Example Implementations

   [Code snippets showing similar functionality]

Use rust-code-mcp tools to extract all information, then format it in a clean, structured way that an AI agent can easily consume.

---
For quick, focused context:

Generate agent context for [TASK] in codebase at [PATH]:

1. Search for: [KEYWORDS]
2. Find definitions and all references for: [SYMBOLS]
3. Get similar code to: [DESCRIPTION]
4. Show call graphs for: [FUNCTIONS]
5. Read relevant files completely

Format output as structured markdown with file paths, line numbers, and code snippets that give complete context for implementing [TASK].

---
For semantic/similarity-based context:

Find all code semantically similar to "[NATURAL_LANGUAGE_DESCRIPTION]" in [PATH].

For each match:
- Show the code snippet with file:line reference
- Explain what it does
- Show its dependencies and references
- Include surrounding context (20 lines before/after)

Use get_similar_code extensively, then enrich results with find_references, get_dependencies, and get_call_graph.
