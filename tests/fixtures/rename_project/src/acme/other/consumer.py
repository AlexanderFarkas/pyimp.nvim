from .old import OtherThing
from . import old
from acme.other.old import OTHER_VALUE


def use_other_consumer():
    return OtherThing, old.OTHER_VALUE, OTHER_VALUE
