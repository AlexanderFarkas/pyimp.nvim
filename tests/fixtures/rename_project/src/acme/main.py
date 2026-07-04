import acme.old
import acme.old as old_alias
from acme.old import Thing
from acme.old import VALUE as OLD_VALUE
from acme.old import *
from acme import old
from acme import old as old_module
from acme import (
    old as old_grouped,
)


def nested_imports():
    import acme.old as nested_old
    from acme.old import Thing as NestedThing
    return nested_old.VALUE, NestedThing


COMMENT = "from acme.old import Nope"
DOC = '''
from acme.old import Nope
'''


def use_everything():
    return (
        acme.old.VALUE,
        old_alias.VALUE,
        Thing,
        OLD_VALUE,
        old.VALUE,
        old_module.VALUE,
        old_grouped.VALUE,
        nested_imports,
    )
