from ...old import Thing
from ... import old
from ..old import PkgThing
from .. import old as pkg_old


def use_deep_consumer():
    from ...old import VALUE
    return Thing, old.VALUE, PkgThing, pkg_old.PKG_VALUE, VALUE
