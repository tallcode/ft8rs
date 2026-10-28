! Calls unchanged upstream GFSK, Watterson, gran and degrade_snr routines.
! No decoder implementation or threshold lives in this fixture generator.
program ft8_channels_probe
  use, intrinsic :: iso_c_binding, only: c_int
  use packjt77
  implicit none
  interface
    subroutine seed_random(seed) bind(C,name='srand')
      import c_int
      integer(c_int), value :: seed
    end subroutine
#ifdef JTDX
    subroutine watterson(c,npts,fs,delay,fspread)
      integer :: npts
      complex :: c(0:npts-1)
      real :: fs,delay,fspread
    end subroutine
#else
    subroutine watterson(c,npts,nsig,fs,delay,fspread)
      integer :: npts,nsig
      complex :: c(0:npts-1)
      real :: fs,delay,fspread
    end subroutine
#endif
  end interface
  character(len=16) :: mode
  character(len=37) :: message, sent
  integer(c_int) :: seed
  integer :: tones(79), i3, n3, npts, first, length
  integer(kind=1) :: bits(77)
  integer(kind=2), allocatable :: pcm(:)
  real :: frequency, delay, spread, db, bw
  real :: unused(151680), real_wave(180000)
  complex :: carrier(151680), signal(0:179999)
  call get_command_argument(1,mode)
  read(*,*) seed
  call seed_random(seed)
  if(trim(mode).eq.'degrade') then
    read(*,*) npts, db, bw
    allocate(pcm(npts))
    open(20,file='input.pcm',access='stream',form='unformatted',status='old')
    read(20) pcm
    close(20)
    do first=1,npts,180000
      length=min(180000,npts-first+1)
#ifdef JTDX
      stop 'degrade_snr is supplied by the WSJT-X reference only'
#else
      call degrade_snr(pcm(first:first+length-1),length,db,bw)
#endif
    enddo
    open(21,file='degraded.pcm',access='stream',form='unformatted',status='replace')
    write(21) pcm
    close(21)
  else
    read(*,'(a)') message
    read(*,*) frequency, delay, spread
#ifdef JTDX
    call genft8(message,i3,n3,1,sent,bits,tones)
#else
    call genft8(message,i3,n3,sent,bits,tones)
#endif
    call gen_ft8wave(tones,79,1920,2.0,12000.0,frequency,carrier,unused,1,151680)
    signal=0.0
    signal(6000:157679)=carrier
#ifdef JTDX
    call watterson(signal,180000,12000.0,delay,spread)
#else
    call watterson(signal,180000,151680,12000.0,delay,spread)
#endif
    real_wave=aimag(signal)
    open(21,file='watterson.f32',access='stream',form='unformatted',status='replace')
    write(21) real_wave
    close(21)
    write(*,'(a)') trim(sent)
  endif
end program
