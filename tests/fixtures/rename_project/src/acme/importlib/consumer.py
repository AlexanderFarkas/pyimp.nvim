from acme.importlib.old import ImportLikeThing
from acme.importlib import old


def use_importlib_consumer():
    return ImportLikeThing, old.IMPORTLIKE_VALUE
