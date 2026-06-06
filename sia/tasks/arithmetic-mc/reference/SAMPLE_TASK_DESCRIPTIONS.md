## Sample Task 1: Arithmetic - Addition

**Question**: What is 17 + 26?

**Options**:
A) 33
B) 43
C) 44
D) 53

**Correct Answer**: B) 43

**Domain**: Arithmetic
**Subdomain**: Addition

---

## Sample Task 2: Arithmetic - Multiplication

**Question**: What is 12 * 8?

**Options**:
A) 86
B) 92
C) 96
D) 108

**Correct Answer**: C) 96

**Domain**: Arithmetic
**Subdomain**: Multiplication

---

## Sample Task 3: Arithmetic - Subtraction

**Question**: What is 100 - 37?

**Options**:
A) 53
B) 63
C) 67
D) 73

**Correct Answer**: B) 63

**Domain**: Arithmetic
**Subdomain**: Subtraction

---

## Format notes

Each item is a single-answer multiple-choice arithmetic question with options
A–D. The target agent must return the chosen letter (the structured-output
schema is `{"answer": "A"|"B"|"C"|"D"}`), and `data/public/evaluate.py` scores
exact-letter accuracy against the held-out answer key. These samples illustrate
the public split; the private split (`data/private/questions.json`) follows the
identical schema with different numbers.
