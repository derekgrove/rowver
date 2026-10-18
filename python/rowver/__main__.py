"""`python -m rowver`: the same as the rowver command."""
from .cli import main

# the guard matters: a spawned worker imports this module as __mp_main__ and must not press again
if __name__ == "__main__":
    main()
