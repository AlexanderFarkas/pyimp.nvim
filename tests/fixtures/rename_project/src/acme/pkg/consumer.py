from ..old import Thing
from .. import old
from .old import PkgThing
from . import old as pkg_old
from acme.pkg.old import PKG_VALUE
from acme.pkg import old as absolute_pkg_old


def use_pkg_consumer():
    from ..old import VALUE
    from . import old as nested_pkg_old
    return (
        Thing,
        old.VALUE,
        PkgThing,
        pkg_old.PKG_VALUE,
        PKG_VALUE,
        absolute_pkg_old.PKG_VALUE,
        VALUE,
        nested_pkg_old.PKG_VALUE,
    )
